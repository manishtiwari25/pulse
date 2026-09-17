package runner

import (
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
)

func TestEnvironmentFailureReason(t *testing.T) {
	cases := []struct {
		name, result, summary, want string
	}{
		{"xcode license exit 69", "exit status 69", "", "environment: xcode-license"},
		{"xcode license text", "Xcode license not accepted", "", "environment: xcode-license"},
		{"xcode in summary", "worker exited 1", "xcode license must be accepted", "environment: xcode-license"},
		{"1password signing", "1Password failed to sign commit", "", "environment: ssh-signing"},
		{"failed to write commit object", "failed to write commit object", "", "environment: ssh-signing"},
		{"gpg signing", "gpg signing failed", "", "environment: ssh-signing"},
		{"missing bundle", "bundle: command not found", "", "environment: missing-bundle"},
		{"missing bundler gem", "cannot load such file -- bundler", "", "environment: missing-bundle"},
		{"missing mise", "mise: command not found", "", "environment: missing-mise"},
		{"missing mise file", "mise: No such file or directory", "", "environment: missing-mise"},
		{"missing executor", "unknown executor hands-1", "", "environment: missing-executor"},
		{"ordinary failure", "verify exit 1", "", ""},
		{"transient failure", "runner error: connection reset", "", ""},
	}
	for _, c := range cases {
		got := environmentFailureReason(c.result, c.summary)
		if got != c.want {
			t.Errorf("%s: got %q, want %q", c.name, got, c.want)
		}
	}
}

func TestRecoverBlocksEnvironmentFailure(t *testing.T) {
	s := newStore(t)
	p, _ := newBareRepo(t, s)
	job := insertRecoverFailedJob(t, s, "job-xcode", "improve", "manual", "exit status 69", plusMinutes(testNow, -30))
	insertRecoverRun(t, s, job.str("id"), "run-xcode", "failed", "exit status 69", "", "")

	lines := recoverFailedJobs(s, p, testNow, "daemon")
	if len(lines) != 1 || !strings.Contains(lines[0], "blocked job-xcode (environment: xcode-license)") {
		t.Fatalf("lines = %v", lines)
	}
	after := jobRow(s.DB, job.str("id"))
	if after.str("status") != "blocked" || after.str("result") != "environment: xcode-license" {
		t.Fatalf("job after block = %v", after)
	}
	detail := recoverDetail(t, s, job.str("id"))
	if detail["outcome"] != "blocked" || detail["class"] != "structural" {
		t.Fatalf("detail = %#v", detail)
	}
	if got := recoverMeta(t, s)["proj"].Note; !strings.Contains(got, "1 blocked") {
		t.Fatalf("note = %q", got)
	}
}

func TestRecoverEnvironmentBlockBackoff(t *testing.T) {
	s := newStore(t)
	p, _ := newBareRepo(t, s)
	job := insertRecoverFailedJob(t, s, "job-backoff", "improve", "manual", "mise: command not found", plusMinutes(testNow, -30))
	insertRecoverRun(t, s, job.str("id"), "run-backoff", "failed", "mise: command not found", "", "")

	recoverFailedJobs(s, p, testNow, "daemon")
	if got := jobRow(s.DB, job.str("id")).str("status"); got != "blocked" {
		t.Fatalf("first block status = %q", got)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = ? AND subject = ? AND json_extract(detail, '$.outcome') = 'blocked'", board.JobRecoverAction, job.str("id")); n != 1 {
		t.Fatalf("block events = %d", n)
	}

	// A second tick within the backoff window must not create another event.
	recoverFailedJobs(s, p, plusMinutes(testNow, 30), "daemon")
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = ? AND subject = ? AND json_extract(detail, '$.outcome') = 'blocked'", board.JobRecoverAction, job.str("id")); n != 1 {
		t.Fatalf("block events after backoff tick = %d", n)
	}

	// After the backoff window the job may be re-examined; it stays blocked.
	recoverFailedJobs(s, p, plusMinutes(testNow, 70), "daemon")
	if got := jobRow(s.DB, job.str("id")).str("status"); got != "blocked" {
		t.Fatalf("status after backoff = %q", got)
	}
}

func TestRecoverPreservesRetryForNonEnvironmentFailures(t *testing.T) {
	s := newStore(t)
	p, _ := newBareRepo(t, s)
	job := insertRecoverFailedJob(t, s, "job-normal", "improve", "manual", "verify exit 1", plusMinutes(testNow, -30))
	insertRecoverRun(t, s, job.str("id"), "run-normal", "failed", "verify exit 1", "", "")

	recoverFailedJobs(s, p, testNow, "daemon")
	if got := jobRow(s.DB, job.str("id")).str("status"); got == "blocked" {
		t.Fatal("non-environment failure was blocked")
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE source = 'manual' AND status = 'queued'"); n != 1 {
		t.Fatalf("queued corrected jobs = %d", n)
	}
}

func TestTakeJobPreflightEnvironmentBlock(t *testing.T) {
	s := newStore(t)
	_, _ = newBareRepo(t, s)

	// Seed a blocked job with an environment failure on the local executor.
	jobID := "job-preflight"
	exec1(s.DB, `INSERT INTO jobs (id, project, kind, tier, title, brief, source, priority, status, created_at, created_by,
		lease_agent, attempts, finished_at, result, branch)
		VALUES (?, 'proj', 'improve', 1, 'Preflight', 'brief', 'manual', 7, 'blocked', ?, 'tester', '', 1, ?, 'environment: xcode-license', '')`,
		jobID, plusHours(testNow, -1), testNow)
	exec1(s.DB, `INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, summary, log, executor)
		VALUES ('run-preflight', ?, 'w1', 'proj', ?, ?, 'failed', 'exit status 69', '', 'local')`,
		jobID, plusMinutes(testNow, -2), testNow)

	// Queue a new job for the same project.
	exec1(s.DB, `INSERT INTO jobs (id, project, kind, tier, title, brief, source, priority, status, created_at, created_by)
		VALUES ('job-next', 'proj', 'improve', 1, 'Next', 'brief', 'manual:next', 7, 'queued', ?, 'tester')`, testNow)

	job, _, why, err := takeJob(s, "w1", "proj", "", false, testNow, localExecutor, 0)
	if err != nil {
		t.Fatal(err)
	}
	if job != nil {
		t.Fatalf("leased a job despite environment block: %v", job)
	}
	if !strings.Contains(why, "environment: xcode-license") {
		t.Fatalf("why = %q", why)
	}

	// After the block ages out, the new job may be leased.
	exec1(s.DB, "UPDATE jobs SET finished_at = ? WHERE id = ?", plusHours(testNow, -2), jobID)
	exec1(s.DB, "UPDATE runs SET finished_at = ? WHERE id = ?", plusHours(testNow, -2), "run-preflight")
	job, _, why, err = takeJob(s, "w1", "proj", "", false, testNow, localExecutor, 0)
	if err != nil {
		t.Fatal(err)
	}
	if job == nil || job.str("id") != "job-next" {
		t.Fatalf("expected job-next, got %v (why=%q)", job, why)
	}
}
