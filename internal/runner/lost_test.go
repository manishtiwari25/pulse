package runner

import (
	"bytes"
	"context"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

// A lost run must not keep its job leased for the rest of minutes_per_job.
func TestSweepLostRunsReleasesLease(t *testing.T) {
	s := newStore(t)
	newBareRepo(t, s)
	join(t, s, "w0")
	job, _, err := addJob(s, "proj", "task", "t", "do it", 1, 0, "test", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	leased, err := leaseJob(s, "w0", testNow, leaseOpts{JobID: job.str("id"), Minutes: 1000})
	if err != nil || leased == nil {
		t.Fatalf("lease: %v %v", leased, err)
	}
	exec1(s.DB, "INSERT INTO runs (id, job_id, agent, project, started_at, status, clone, log, mode, executor) VALUES ('run-1', ?, 'w0', 'proj', ?, 'running', '', '', 'real', 'local')",
		job.str("id"), testNow)
	// a finished run of another job is never touched
	exec1(s.DB, "INSERT INTO runs (id, job_id, agent, project, started_at, status, clone, log, mode, executor) VALUES ('run-0', 'job-other', 'w0', 'proj', ?, 'done', '', '', 'real', 'local')", testNow)

	if n := sweepLostRuns(s, testNow); n != 1 {
		t.Fatalf("swept %d runs, want 1", n)
	}
	if got := mustOne(s.DB, "SELECT status FROM runs WHERE id = 'run-1'").str("status"); got != "lost" {
		t.Fatalf("run status %q", got)
	}
	j := jobRow(s.DB, job.str("id"))
	if j.str("status") != "queued" || j.str("lease_agent") != "" || j.str("lease_until") != "" {
		t.Fatalf("lost run left its job %s by %q until %q", j.str("status"), j.str("lease_agent"), j.str("lease_until"))
	}
	if count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'job.requeue' AND subject = ?", job.str("id")) != 1 {
		t.Fatal("no job.requeue event")
	}

	// Lost for the maxAttempts-th time: the job fails instead of cycling forever.
	exec1(s.DB, "UPDATE jobs SET status = 'leased', lease_agent = 'w0', lease_until = '2099-01-01T00:00:00Z', attempts = ? WHERE id = ?", maxAttempts, job.str("id"))
	exec1(s.DB, "INSERT INTO runs (id, job_id, agent, project, started_at, status, clone, log, mode, executor) VALUES ('run-2', ?, 'w0', 'proj', ?, 'running', '', '', 'real', 'local')",
		job.str("id"), testNow)
	if n := sweepLostRuns(s, testNow); n != 1 {
		t.Fatalf("swept %d runs, want 1", n)
	}
	j = jobRow(s.DB, job.str("id"))
	if j.str("status") != "failed" || !strings.Contains(j.str("result"), "3 times") || j.str("lease_agent") != "" {
		t.Fatalf("third loss: %s %q by %q", j.str("status"), j.str("result"), j.str("lease_agent"))
	}

	// A lease that moved on to another worker is not this run's to release.
	exec1(s.DB, "UPDATE jobs SET status = 'leased', lease_agent = 'w1', attempts = 1 WHERE id = ?", job.str("id"))
	exec1(s.DB, "INSERT INTO runs (id, job_id, agent, project, started_at, status, clone, log, mode, executor) VALUES ('run-3', ?, 'w0', 'proj', ?, 'running', '', '', 'real', 'local')",
		job.str("id"), testNow)
	sweepLostRuns(s, testNow)
	if j = jobRow(s.DB, job.str("id")); j.str("status") != "leased" || j.str("lease_agent") != "w1" {
		t.Fatalf("another worker's lease was released: %s by %q", j.str("status"), j.str("lease_agent"))
	}
}

func TestReportReported(t *testing.T) {
	text := "Error: Authentication token found but could not be validated.\n\nTo authenticate, you can use any of the following methods:\n"
	if r := parseResultText(&text); !r.Found || r.reported() {
		t.Fatalf("harness error text: found %v reported %v", r.Found, r.reported())
	}
	for _, line := range []string{"SUMMARY: did it", "- `FINDING`: a thing", "VOTE: 20260907T1-x up fine", "QUESTION: which one?", "SUGGESTION: area/thread - card"} {
		l := line
		if r := parseResultText(&l); !r.reported() {
			t.Fatalf("%q not seen as a report", line)
		}
	}
	if parseResultText(nil).reported() {
		t.Fatal("nil text reported")
	}
}

// A harness that dies at launch (here: the Copilot login is rejected) is a runner
// error, not the job's failure: the job is queued again and the summary names the
// login on this machine.
func TestLaunchRejectionOnLocalIsRunnerError(t *testing.T) {
	s := newStore(t)
	newBareRepo(t, s)
	join(t, s, "aurora")
	job, _, err := addJob(s, "proj", "task", "t", "do it", 1, 0, "test", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	cmd := "sh -c 'echo \"Error: Authentication token found but could not be validated.\" >&2; exit 1'"
	r, reason, err := workOnce(s, "aurora", "", "", testNow, runOpts{WorkerCmd: cmd})
	if err != nil || r == nil {
		t.Fatalf("work: %v %q %v", r, reason, err)
	}
	if r.str("status") != "error" || !strings.Contains(r.str("summary"), "rejected at launch") || !strings.Contains(r.str("summary"), "login on this machine") {
		t.Fatalf("run %s: %s", r.str("status"), r.str("summary"))
	}
	j := jobRow(s.DB, job.str("id"))
	if j.str("status") != "queued" {
		t.Fatalf("job %s after a launch rejection: %s", j.str("status"), j.str("result"))
	}
	// the same rejection with a slow worker is still the job's failure
	slow := "sh -c 'sleep 0.2; echo \"Error: Authentication token found but could not be validated.\" >&2; exit 1'"
	oldClock := wallClock
	base := time.Now()
	calls := 0
	wallClock = func() time.Time {
		calls++
		return base.Add(time.Duration(calls) * launchSeconds * time.Second)
	}
	t.Cleanup(func() { wallClock = oldClock })
	r, _, err = workOnce(s, "aurora", "", "", testNow, runOpts{WorkerCmd: slow})
	if err != nil || r == nil || r.str("status") != "failed" || r.str("summary") != "worker exited 1" {
		t.Fatalf("slow rejection: %v %v", r, err)
	}
}

func TestNativeNotLoggedInExitFourIsRunnerErrorAfterSlowStart(t *testing.T) {
	s := newStore(t)
	newBareRepo(t, s)
	join(t, s, "aurora")
	job, _, err := addJob(s, "proj", "task", "t", "do it", 1, 0, "test", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	oldClock := wallClock
	base := time.Now()
	calls := 0
	wallClock = func() time.Time {
		calls++
		return base.Add(time.Duration(calls) * launchSeconds * time.Second)
	}
	t.Cleanup(func() { wallClock = oldClock })
	r, reason, err := workOnce(s, "aurora", "", "", testNow, runOpts{WorkerCmd: "sh -c 'echo \"not logged in: sign in first\" >&2; exit 4'"})
	if err != nil || r == nil {
		t.Fatalf("work: %v %q %v", r, reason, err)
	}
	if r.str("status") != "error" || !strings.Contains(r.str("summary"), "provider login on this machine") {
		t.Fatalf("run %s: %s", r.str("status"), r.str("summary"))
	}
	if j := jobRow(s.DB, job.str("id")); j.str("status") != "queued" || j.int("attempts") != 0 {
		t.Fatalf("job after login fault: %s attempts=%d", j.str("status"), j.int("attempts"))
	}
}

func TestWorkerExitSummaryIncludesHarnessStopReason(t *testing.T) {
	s := newStore(t)
	newBareRepo(t, s)
	join(t, s, "aurora")
	_, _, err := addJob(s, "proj", "task", "t", "do it", 1, 0, "test", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	r, reason, err := workOnce(s, "aurora", "", "", testNow, runOpts{WorkerCmd: "sh -c 'echo \"SUMMARY: harness stopped: step_limit after 2 steps\"; exit 1'"})
	if err != nil || r == nil {
		t.Fatalf("work: %v %q %v", r, reason, err)
	}
	want := "worker exited 1: harness stopped: step_limit after 2 steps"
	if r.str("status") != "failed" || r.str("summary") != want {
		t.Fatalf("run %s: %s, want failed: %s", r.str("status"), r.str("summary"), want)
	}
}

func TestWorkerExitFiveIsOwnerCancelledFailure(t *testing.T) {
	s := newStore(t)
	newBareRepo(t, s)
	join(t, s, "aurora")
	job, _, err := addJob(s, "proj", "task", "t", "do it", 1, 0, "test", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	r, reason, err := workOnce(s, "aurora", "", "", testNow, runOpts{WorkerCmd: "sh -c 'echo \"SUMMARY: stopped after cancel\"; exit 5'"})
	if err != nil || r == nil {
		t.Fatalf("work: %v %q %v", r, reason, err)
	}
	if r.str("status") != "failed" || r.str("summary") != "cancelled by the owner: stopped after cancel" || r.int("exit_code") != 5 {
		t.Fatalf("run after cancel exit: %v", r)
	}
	if j := jobRow(s.DB, job.str("id")); j.str("status") != "failed" || j.str("result") != "cancelled by the owner: stopped after cancel" {
		t.Fatalf("job after cancel exit: %v", j)
	}
}

func TestFailureWorkerExitFiveKeepsPlainCancelSummary(t *testing.T) {
	s := newStore(t)
	newBareRepo(t, s)
	join(t, s, "aurora")
	job, _, err := addJob(s, "proj", "task", "t", "do it", 1, 0, "test", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	want := "harness stopped: cancelled after 1 model calls and 0 commands; error: interrupted: context canceled"
	r, reason, err := workOnce(s, "aurora", "", "", testNow, runOpts{WorkerCmd: "sh -c 'echo \"SUMMARY: " + want + "\"; exit 5'"})
	if err != nil || r == nil {
		t.Fatalf("work: %v %q %v", r, reason, err)
	}
	if r.str("status") != "failed" || r.str("summary") != want || r.int("exit_code") != 5 {
		t.Fatalf("run after plain cancel exit: %v", r)
	}
	if j := jobRow(s.DB, job.str("id")); j.str("status") != "failed" || j.str("result") != want {
		t.Fatalf("job after plain cancel exit: %v", j)
	}
}

func TestRunHarnessCancelWatcherCancelsContext(t *testing.T) {
	s := newStore(t)
	if err := s.BeginHarnessSession(store.HarnessSession{ID: "run-cancel"}); err != nil {
		t.Fatal(err)
	}
	ctx, stop := context.WithCancel(context.Background())
	defer stop()
	workerCtx, cancel := context.WithCancel(ctx)
	var ownerCancelled atomic.Bool
	done := watchRunHarnessCancelEvery(ctx, s, "run-cancel", cancel, &ownerCancelled, time.Millisecond)
	defer done()
	if ok, err := s.RequestHarnessCancel("run-cancel"); err != nil || !ok {
		t.Fatalf("request cancel = %v %v", ok, err)
	}
	select {
	case <-workerCtx.Done():
	case <-time.After(time.Second):
		t.Fatal("worker context was not cancelled")
	}
	if !ownerCancelled.Load() {
		t.Fatal("owner cancellation flag was not set")
	}
}

// On remote hands the same rejection is the executor's fault: it drains, the job
// keeps its attempt.
func TestLaunchRejectionOnRemoteDrainsExecutor(t *testing.T) {
	skipSlow(t)
	if _, err := exec.LookPath("ssh-keygen"); err != nil {
		t.Skip("no ssh-keygen")
	}
	bin := buildPulse(t)
	stageLocalPulse(t, bin)
	s := newStore(t)
	newBareRepo(t, s)
	join(t, s, "aurora")
	if err := s.SetSetting(humanAgent, "minutes_per_job", "1"); err != nil {
		t.Fatal(err)
	}
	workdir := filepath.Join(t.TempDir(), "srv")
	handsHome := t.TempDir()
	handsPath := fakeHandsPath(t)
	old := runSSH
	t.Cleanup(func() { runSSH = old })
	runSSH = func(_ context.Context, argv []string, stdin []byte, timeout time.Duration) (int, string, string, error) {
		cmd := exec.Command(bin)
		cmd.Env = append(os.Environ(), "PULSE_TEST_HANDS=1", "PULSE_HANDS_WORKDIR="+workdir, "HOME="+handsHome, handsPath,
			"PULSE_HANDS_ALLOW_ANY_WORKER=1", "PULSE_HANDS_KEYS_OK=yes", "SSH_ORIGINAL_COMMAND="+argv[len(argv)-1])
		cmd.Stdin = bytes.NewReader(stdin)
		var out, errb bytes.Buffer
		cmd.Stdout, cmd.Stderr = &out, &errb
		err := cmd.Run()
		if ee, ok := err.(*exec.ExitError); ok {
			return ee.ExitCode(), out.String(), errb.String(), nil
		}
		return 0, out.String(), errb.String(), err
	}
	if _, err := addExecutor(s, testNow, "hands", "pulse@hands", workdir, 1, "*", humanAgent); err != nil {
		t.Fatal(err)
	}
	if ok, note, err := probeExecutor(s, "hands", humanAgent, testNow); err != nil || !ok {
		t.Fatalf("probe: %v %q %v", ok, note, err)
	}
	job, _, err := addJob(s, "proj", "task", "remote", "say hi remotely", 1, 0, "test", humanAgent, false)
	if err != nil {
		t.Fatal(err)
	}
	cmd := "sh -c 'echo \"Error: Authentication token found but could not be validated. GitHub returned: Sorry. Your account was suspended\" >&2; exit 1'"
	r, reason, err := workOnce(s, "aurora", "", "", testNow, runOpts{Executor: "hands", WorkerCmd: cmd})
	if err != nil || r == nil {
		t.Fatalf("work: %v %q %v", r, reason, err)
	}
	if r.str("status") != "error" || !strings.Contains(r.str("summary"), "the login on hands, not the job") {
		t.Fatalf("run %s: %s", r.str("status"), r.str("summary"))
	}
	j := jobRow(s.DB, job.str("id"))
	if j.str("status") != "queued" || j.int("attempts") != 0 {
		t.Fatalf("job %s attempts %d after an executor fault", j.str("status"), j.int("attempts"))
	}
	ex := executorRow(s, "hands")
	if ex.str("status") != "drained" || !strings.Contains(ex.str("probe_note"), "fault:") {
		t.Fatalf("executor not drained: %s %q", ex.str("status"), ex.str("probe_note"))
	}
}
