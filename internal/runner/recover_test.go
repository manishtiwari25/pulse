package runner

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
)

type recoveryMetaNote struct {
	At   string `json:"at"`
	Note string `json:"note"`
}

func setRecoverSetting(t *testing.T, s *store.Store, key, value string) {
	t.Helper()
	if err := s.SetSetting(humanAgent, key, value); err != nil {
		t.Fatal(err)
	}
}

func insertRecoverFailedJob(t *testing.T, s *store.Store, id, kind, source, result, finished string) row {
	t.Helper()
	exec1(s.DB, `INSERT INTO jobs (id, project, kind, tier, title, brief, source, priority, status, created_at, created_by,
		lease_agent, attempts, finished_at, result, branch)
		VALUES (?, 'proj', ?, 1, ?, ?, ?, 7, 'failed', ?, 'tester', 'w1', 3, ?, ?, ?)`,
		id, kind, "Original "+id, "original brief "+id, source, plusHours(finished, -1), finished, result, "pulse/w1/"+id)
	return jobRow(s.DB, id)
}

func insertRecoverRun(t *testing.T, s *store.Store, jobID, runID, status, summary, logPath, branch string) row {
	t.Helper()
	exec1(s.DB, `INSERT INTO runs (id, job_id, agent, project, started_at, finished_at, status, branch, summary, log, executor)
		VALUES (?, ?, 'w1', 'proj', ?, ?, ?, ?, ?, ?, 'local')`,
		runID, jobID, plusHours(testNow, -2), plusHours(testNow, -1), status, branch, summary, logPath)
	return mustOne(s.DB, "SELECT * FROM runs WHERE id = ?", runID)
}

func recoverMeta(t *testing.T, s *store.Store) map[string]recoveryMetaNote {
	t.Helper()
	r := mustOne(s.DB, "SELECT value FROM meta WHERE key = 'recovery'")
	if r == nil {
		return nil
	}
	out := map[string]recoveryMetaNote{}
	if err := json.Unmarshal([]byte(r.str("value")), &out); err != nil {
		t.Fatal(err)
	}
	return out
}

func recoverDetail(t *testing.T, s *store.Store, jobID string) map[string]any {
	t.Helper()
	r := mustOne(s.DB, "SELECT detail FROM events WHERE action = ? AND subject = ? ORDER BY seq DESC LIMIT 1", board.JobRecoverAction, jobID)
	if r == nil {
		t.Fatalf("no recovery event for %s", jobID)
	}
	return jsonMap(r.str("detail"))
}

func TestRecoverOffSwitchWritesNoteOnlyWithFailures(t *testing.T) {
	s := newStore(t)
	p, _ := newRepo(t, s)
	setRecoverSetting(t, s, "daemon.recover", "off")
	insertRecoverFailedJob(t, s, "job-off", "task", "manual", "verify exit 1", plusMinutes(testNow, -30))
	lines := recoverFailedJobs(s, p, testNow, "daemon")
	if len(lines) != 0 {
		t.Fatalf("lines = %v", lines)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = ?", board.JobRecoverAction); n != 0 {
		t.Fatalf("recover events = %d", n)
	}
	if got := recoverMeta(t, s)["proj"].Note; got != "recovery off (daemon.recover)" {
		t.Fatalf("note = %q", got)
	}

	exec1(s.DB, "UPDATE jobs SET status = 'done' WHERE id = 'job-off'")
	recoverFailedJobs(s, p, testNow, "daemon")
	if _, ok := recoverMeta(t, s)["proj"]; ok {
		t.Fatal("recovery note kept for a project with no failed jobs")
	}
}

func TestRecoverTransientWaitsThenRequeuesAndDropsBranch(t *testing.T) {
	s := newStore(t)
	p, root := newRepo(t, s)
	job := insertRecoverFailedJob(t, s, "job-transient", "task", "manual", "runner error: connection reset", plusMinutes(testNow, -2))
	branch := "pulse/w1/" + job.str("id")
	run(t, root, "git", "branch", branch)
	insertRecoverRun(t, s, job.str("id"), "run-transient", "error", "runner error: connection reset", "", branch)

	recoverFailedJobs(s, p, testNow, "daemon")
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = ?", board.JobRecoverAction); n != 0 {
		t.Fatalf("events before due = %d", n)
	}
	if got := recoverMeta(t, s)["proj"].Note; !strings.Contains(got, "next at "+recoverDue(job.str("finished_at"), 0)) {
		t.Fatalf("note before due = %q", got)
	}

	lines := recoverFailedJobs(s, p, plusMinutes(testNow, 4), "daemon")
	if len(lines) != 1 || !strings.Contains(lines[0], "recovered job-transient (transient): queued again after a 5m pause") {
		t.Fatalf("lines = %v", lines)
	}
	again := jobRow(s.DB, job.str("id"))
	if again.str("status") != "queued" || again.int("attempts") != 0 || again.str("branch") != "" {
		t.Fatalf("job after recovery = %v", again)
	}
	if git(root, "rev-parse", "--verify", "--quiet", "refs/heads/"+branch).ok() {
		t.Fatalf("stale branch %s still exists", branch)
	}
	detail := recoverDetail(t, s, job.str("id"))
	if detail["outcome"] != "requeued" || detail["class"] != "transient" || detail["root"] != job.str("id") || detail["n"].(float64) != 1 {
		t.Fatalf("detail = %#v", detail)
	}
}

func TestRecoverDeterministicNonTaskQueuesCorrectedJobWithLogTail(t *testing.T) {
	s := newStore(t)
	p, root := newRepo(t, s)
	job := insertRecoverFailedJob(t, s, "job-det", "improve", "manual", "verify exit 1", plusMinutes(testNow, -30))
	logPath := filepath.Join(root, "run-det.log")
	var lines []string
	for i := 0; i < 45; i++ {
		lines = append(lines, "log line "+string(rune('A'+i%26)))
	}
	lines = append(lines, "last important github_pat_abcdefghijklmnopqrstuvwxyz")
	if err := os.WriteFile(logPath, []byte(strings.Join(lines, "\n")), 0o644); err != nil {
		t.Fatal(err)
	}
	insertRecoverRun(t, s, job.str("id"), "run-det", "failed", "verify exit 1", logPath, "")

	out := recoverFailedJobs(s, p, testNow, "daemon")
	if len(out) != 1 || !strings.Contains(out[0], "re-planned job-det as") {
		t.Fatalf("lines = %v", out)
	}
	copy := mustOne(s.DB, "SELECT * FROM jobs WHERE source = 'manual' AND status = 'queued'")
	if copy == nil {
		t.Fatal("corrected job missing")
	}
	if copy.str("status") != "queued" || !strings.HasPrefix(copy.str("title"), "Retry job-det: Original job-det") {
		t.Fatalf("copy = %v", copy)
	}
	brief := copy.str("brief")
	for _, want := range []string{"original brief job-det", "verify exit 1", "Job `job-det` (run `run-det`, executor `local`)", "last important", "[redacted]"} {
		if !strings.Contains(brief, want) {
			t.Fatalf("brief missing %q:\n%s", want, brief)
		}
	}
	if strings.Contains(brief, "github_pat_abcdefghijklmnopqrstuvwxyz") {
		t.Fatal("brief leaked a token-shaped value")
	}
	detail := recoverDetail(t, s, job.str("id"))
	if detail["outcome"] != "replanned" || detail["job"] != copy.str("id") || detail["n"].(float64) != 1 {
		t.Fatalf("detail = %#v", detail)
	}
}

func TestRecoverCorrectedChainKeepsRootAndBackoff(t *testing.T) {
	s := newStore(t)
	p, _ := newRepo(t, s)
	insertRecoverFailedJob(t, s, "job-root", "improve", "manual", "verify exit 1", plusHours(testNow, -2))
	eventAt(s.DB, plusHours(testNow, -1), "daemon", board.JobRecoverAction, "job-root",
		map[string]any{"outcome": "replanned", "class": "deterministic", "why": "verify exit 1", "root": "job-root", "n": 1, "signature": "deterministic:verify exit #", "job": "job-corrected"})
	corrected := insertRecoverFailedJob(t, s, "job-corrected", "improve", "manual", "verify exit 1", plusMinutes(testNow, -5))
	insertRecoverRun(t, s, corrected.str("id"), "run-corrected", "failed", "verify exit 1", "", "")

	recoverFailedJobs(s, p, testNow, "daemon")
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = ? AND subject = 'job-corrected'", board.JobRecoverAction); n != 0 {
		t.Fatalf("events before second backoff = %d", n)
	}
	if got := recoverMeta(t, s)["proj"].Note; !strings.Contains(got, "next at "+recoverDue(corrected.str("finished_at"), 1)) {
		t.Fatalf("note = %q", got)
	}

	recoverFailedJobs(s, p, plusMinutes(testNow, 11), "daemon")
	detail := recoverDetail(t, s, corrected.str("id"))
	if detail["root"] != "job-root" || detail["n"].(float64) != 2 {
		t.Fatalf("detail = %#v", detail)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE source = 'manual' AND status = 'queued'"); n != 1 {
		t.Fatalf("new corrected queued jobs = %d", n)
	}
	if got := jobRow(s.DB, detail["job"].(string)).str("title"); got != "Retry job-root: Original job-corrected" {
		t.Fatalf("copy title = %q", got)
	}
}

func TestRecoverDedupeOpenCorrectedJob(t *testing.T) {
	s := newStore(t)
	p, _ := newRepo(t, s)
	insertRecoverFailedJob(t, s, "job-dedupe", "improve", "manual", "verify exit 1", plusMinutes(testNow, -30))
	exec1(s.DB, `INSERT INTO jobs (id, project, kind, tier, title, brief, source, status, created_at)
		VALUES ('job-open-recover', 'proj', 'improve', 1, 'open', 'open', 'manual', 'queued', ?)`, testNow)

	recoverFailedJobs(s, p, testNow, "daemon")
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE source = 'manual'"); n != 2 {
		t.Fatalf("jobs with the source = %d", n)
	}
	detail := recoverDetail(t, s, "job-dedupe")
	if detail["outcome"] != "held" || detail["why"] != "superseded by job-open-recover" {
		t.Fatalf("detail = %#v", detail)
	}
}

func TestRecoverTaskOwnedDeterministicWaitsForLead(t *testing.T) {
	s := newStore(t)
	p, _ := newRepo(t, s)
	job := insertRecoverFailedJob(t, s, "job-task", "task", "task:TASK-1", "verify exit 1", plusMinutes(testNow, -30))
	recoverFailedJobs(s, p, testNow, "daemon")
	after := jobRow(s.DB, job.str("id"))
	if after.str("status") != "failed" {
		t.Fatalf("task-owned job changed: %v", after)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = ?", board.JobRecoverAction); n != 0 {
		t.Fatalf("recover events = %d", n)
	}
	if got := recoverMeta(t, s)["proj"].Note; !strings.Contains(got, "with the lead") {
		t.Fatalf("note = %q", got)
	}
}

func TestRecoverStructuralHeldWithoutFinding(t *testing.T) {
	s := newStore(t)
	p, _ := newRepo(t, s)
	job := insertRecoverFailedJob(t, s, "job-struct", "task", "manual", "needs a real decision", plusMinutes(testNow, -1))
	recoverFailedJobs(s, p, testNow, "daemon")
	after := jobRow(s.DB, job.str("id"))
	if after.str("status") != "failed" {
		t.Fatalf("structural job changed: %v", after)
	}
	detail := recoverDetail(t, s, job.str("id"))
	if detail["outcome"] != "held" || detail["class"] != "structural" {
		t.Fatalf("detail = %#v", detail)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding'"); n != 0 {
		t.Fatalf("finding posts = %d", n)
	}
}

func TestRecoverLimitHeldPostsOneFindingAcrossTicks(t *testing.T) {
	s := newStore(t)
	p, _ := newRepo(t, s)
	setRecoverSetting(t, s, "daemon.recover_limit", "1")
	insertRecoverFailedJob(t, s, "job-limit", "improve", "manual", "verify exit 1", plusMinutes(testNow, -40))
	insertRecoverFailedJob(t, s, "job-limit-copy", "improve", "manual", "verify exit 1", plusMinutes(testNow, -39))
	eventAt(s.DB, plusHours(testNow, -2), "daemon", board.JobRecoverAction, "job-old",
		map[string]any{"outcome": "replanned", "class": "deterministic", "why": "verify exit 1", "root": "job-limit", "n": 1, "signature": "deterministic:verify exit #", "job": "job-limit-copy"})

	recoverFailedJobs(s, p, testNow, "daemon")
	recoverFailedJobs(s, p, plusMinutes(testNow, 1), "daemon")
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = ? AND json_extract(detail, '$.outcome') = 'held'", board.JobRecoverAction); n != 2 {
		t.Fatalf("held events = %d", n)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding' AND area = 'jobs' AND thread = 'job-limit'"); n != 1 {
		t.Fatalf("finding posts = %d", n)
	}
	post := mustOne(s.DB, "SELECT body FROM posts WHERE kind = 'finding' AND thread = 'job-limit'")
	if !strings.Contains(post.str("body"), "pulse job retry job-limit --as-new") {
		t.Fatalf("finding body = %q", post.str("body"))
	}
}

func TestRecoverSkipsOwnedKindsAndOldWindow(t *testing.T) {
	s := newStore(t)
	p, _ := newRepo(t, s)
	for _, tc := range []struct {
		id, kind string
	}{
		{"job-lead", "lead"},
		{"job-baseline", "baseline"},
		{"job-experiment", "experiment"},
	} {
		insertRecoverFailedJob(t, s, tc.id, tc.kind, "manual", "runner error", plusMinutes(testNow, -30))
	}
	insertRecoverFailedJob(t, s, "job-old", "task", "manual", "runner error", plusHours(testNow, -24*8))

	recoverFailedJobs(s, p, testNow, "daemon")
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = ?", board.JobRecoverAction); n != 0 {
		t.Fatalf("recover events = %d", n)
	}
	if _, ok := recoverMeta(t, s)["proj"]; ok {
		t.Fatal("note written for skipped jobs")
	}
}

func TestRecoverPerTickCap(t *testing.T) {
	s := newStore(t)
	p, _ := newRepo(t, s)
	for i := 0; i < 6; i++ {
		id := "job-cap-" + string(rune('a'+i))
		insertRecoverFailedJob(t, s, id, "task", "manual", "runner error", plusMinutes(testNow, -30))
	}
	recoverFailedJobs(s, p, testNow, "daemon")
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = ?", board.JobRecoverAction); n != recoverPerTick {
		t.Fatalf("recover events = %d", n)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE status = 'queued'"); n != recoverPerTick {
		t.Fatalf("queued jobs = %d", n)
	}
	if got := recoverMeta(t, s)["proj"].Note; !strings.Contains(got, "cap 5 reached") {
		t.Fatalf("note = %q", got)
	}
}

func TestRecoverMetaShapeAndRemovalPreservesOtherProjects(t *testing.T) {
	s := newStore(t)
	p, _ := newRepo(t, s)
	exec1(s.DB, "INSERT INTO meta (key, value) VALUES ('recovery', ?)", `{"other":{"at":"2026-09-06T09:00:00Z","note":"kept"}}`)
	insertRecoverFailedJob(t, s, "job-meta", "task", "manual", "runner error", plusMinutes(testNow, -30))
	recoverFailedJobs(s, p, testNow, "daemon")
	notes := recoverMeta(t, s)
	if notes["proj"].At != testNow || !strings.Contains(notes["proj"].Note, "1 failed in 7d") {
		t.Fatalf("proj note = %#v", notes["proj"])
	}
	if notes["other"].Note != "kept" {
		t.Fatalf("other note lost: %#v", notes)
	}

	exec1(s.DB, "UPDATE jobs SET status = 'done' WHERE id = 'job-meta'")
	recoverFailedJobs(s, p, plusMinutes(testNow, 1), "daemon")
	notes = recoverMeta(t, s)
	if _, ok := notes["proj"]; ok || notes["other"].Note != "kept" {
		t.Fatalf("notes after removal = %#v", notes)
	}
}

func TestRecoverIdempotentForHandledFailure(t *testing.T) {
	s := newStore(t)
	p, _ := newRepo(t, s)
	insertRecoverFailedJob(t, s, "job-idem", "improve", "manual", "verify exit 1", plusMinutes(testNow, -30))
	recoverFailedJobs(s, p, testNow, "daemon")
	recoverFailedJobs(s, p, plusMinutes(testNow, 1), "daemon")
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = ?", board.JobRecoverAction); n != 1 {
		t.Fatalf("recover events = %d", n)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE source = 'manual' AND status = 'queued'"); n != 1 {
		t.Fatalf("recover jobs = %d", n)
	}
}

func TestRecoverSupersededFailureIsHeldOnce(t *testing.T) {
	s := newStore(t)
	p, _ := newRepo(t, s)
	failed := insertRecoverFailedJob(t, s, "job-old", "improve", "suggestion:POST-1", "runner error: connection reset", plusHours(testNow, -3))
	exec1(s.DB, `INSERT INTO jobs (id, project, kind, tier, title, brief, source, priority, status, created_at, created_by)
		VALUES ('job-later', 'proj', 'improve', 1, 'Later carrier', 'brief', 'suggestion:POST-1', 0, 'done', ?, 'lead')`, plusHours(testNow, -2))

	lines := recoverFailedJobs(s, p, testNow, "daemon")
	if len(lines) != 0 {
		t.Fatalf("lines = %v", lines)
	}
	if got := jobRow(s.DB, failed.str("id")).str("status"); got != "failed" {
		t.Fatalf("superseded job status = %q, want failed", got)
	}
	detail := recoverDetail(t, s, failed.str("id"))
	if detail["outcome"] != "held" || detail["why"] != "superseded by job-later" || detail["job"] != "job-later" {
		t.Fatalf("detail = %#v", detail)
	}
	if got := recoverMeta(t, s)["proj"].Note; got != "1 failed in 7d: 1 held" {
		t.Fatalf("note = %q", got)
	}
	recoverFailedJobs(s, p, plusMinutes(testNow, 1), "daemon")
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = ? AND subject = 'job-old'", board.JobRecoverAction); n != 1 {
		t.Fatalf("held events = %d, want 1", n)
	}
}

func TestRecoverTaskOwnedTransientDefersToLeadFollowUp(t *testing.T) {
	s := newStore(t)
	p, _ := newRepo(t, s)
	job := insertRecoverFailedJob(t, s, "job-task", "task", "task:TASK-1", "runner error: connection reset", plusHours(testNow, -1))
	if !recoveryPending(s, job.str("id")) {
		t.Fatal("a fresh transient task failure should be pending recovery")
	}
	eventAt(s.DB, plusMinutes(testNow, -30), "daemon", followUpAction, "TASK-1", map[string]any{"job": "lead-1", "why": "failed"})

	recoverFailedJobs(s, p, testNow, "daemon")
	if got := jobRow(s.DB, job.str("id")).str("status"); got != "failed" {
		t.Fatalf("status = %q, want failed (the lead has it)", got)
	}
	if got := recoverMeta(t, s)["proj"].Note; !strings.Contains(got, "1 with the lead") {
		t.Fatalf("note = %q", got)
	}
}

func TestRecoveryPendingOnlyForUnansweredTransientWithinLimit(t *testing.T) {
	s := newStore(t)
	newRepo(t, s)
	insertRecoverFailedJob(t, s, "job-det", "task", "task:TASK-2", "verify exit 1", plusHours(testNow, -1))
	if recoveryPending(s, "job-det") {
		t.Fatal("a deterministic failure is the lead's, not pending recovery")
	}
	insertRecoverFailedJob(t, s, "job-tr", "task", "task:TASK-3", "runner error: connection reset", plusHours(testNow, -1))
	if !recoveryPending(s, "job-tr") {
		t.Fatal("transient failure should be pending")
	}
	eventAt(s.DB, testNow, "daemon", board.JobRecoverAction, "job-tr",
		map[string]any{"outcome": "held", "class": "transient", "why": "recovered 3 times", "root": "job-tr", "n": 3, "signature": "transient:runner error"})
	if recoveryPending(s, "job-tr") {
		t.Fatal("an answered failure is no longer pending")
	}
	setRecoverSetting(t, s, "daemon.recover", "off")
	insertRecoverFailedJob(t, s, "job-off", "task", "task:TASK-4", "runner error: connection reset", plusHours(testNow, -1))
	if recoveryPending(s, "job-off") {
		t.Fatal("nothing is pending with recovery off")
	}
}

func TestRetryTitlePrefixesOnce(t *testing.T) {
	if got := retryTitle("job-1", "Fix the build"); got != "Retry job-1: Fix the build" {
		t.Fatalf("got %q", got)
	}
	if got := retryTitle("job-1", "Retry job-1: Fix the build"); got != "Retry job-1: Fix the build" {
		t.Fatalf("got %q", got)
	}
}

func TestRecoverWaitsWhileTheQueueIsFull(t *testing.T) {
	s := newStore(t)
	p, _ := newRepo(t, s)
	for i := 0; i < recoverQueueRoom; i++ {
		exec1(s.DB, `INSERT INTO jobs (id, project, kind, tier, title, brief, source, status, created_at)
			VALUES (?, 'proj', 'improve', 1, 'fresh', 'fresh', ?, 'queued', ?)`, fmt.Sprintf("job-fresh-%d", i), fmt.Sprintf("fresh:%d", i), testNow)
	}
	insertRecoverFailedJob(t, s, "job-waits", "improve", "manual", "runner error: connection reset", plusMinutes(testNow, -30))
	insertRecoverFailedJob(t, s, "job-held", "improve", "manual-2", "1 commit(s) touch owner-owned paths (.github/x.yml); nothing landed", plusMinutes(testNow, -30))

	lines := recoverFailedJobs(s, p, testNow, "daemon")
	if len(lines) != 1 || !strings.Contains(lines[0], "held job-held (structural)") {
		t.Fatalf("lines = %v", lines)
	}
	if got := jobRow(s.DB, "job-waits").str("status"); got != "failed" {
		t.Fatalf("status = %q; recovery added to a full queue", got)
	}
	if got := recoverMeta(t, s)["proj"].Note; !strings.Contains(got, fmt.Sprintf("waits for the queue (%d queued)", recoverQueueRoom)) {
		t.Fatalf("note = %q", got)
	}

	exec1(s.DB, "UPDATE jobs SET status = 'done' WHERE id LIKE 'job-fresh-%'")
	recoverFailedJobs(s, p, plusMinutes(testNow, 1), "daemon")
	if got := jobRow(s.DB, "job-waits").str("status"); got != "queued" {
		t.Fatalf("status = %q after the queue drained", got)
	}
}
