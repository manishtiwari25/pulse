package runner

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
)

func recoveryWithoutPanic(t *testing.T, fn func() (string, bool)) (line string, acted bool) {
	t.Helper()
	defer func() {
		if p := recover(); p != nil {
			t.Errorf("recovery panicked instead of reporting the failed write: %v", p)
		}
	}()
	return fn()
}

func TestRecoveryResilienceFailedWriteIsAtomic(t *testing.T) {
	for _, tc := range []struct {
		class  failureClass
		result string
		action string
	}{
		{failureTransient, "runner error: connection reset", "job.queued"},
		{failureTransient, "runner error: connection reset", board.JobRecoverAction},
		{failureDeterministic, "verify exit 1", "job.add"},
		{failureDeterministic, "verify exit 1", board.JobRecoverAction},
	} {
		t.Run(string(tc.class)+"/"+tc.action, func(t *testing.T) {
			s := newStore(t)
			p, root := newRepo(t, s)
			job := insertRecoverFailedJob(t, s, "job-write-failure", "improve", "", tc.result, plusMinutes(testNow, -30))
			run(t, root, "git", "branch", job.str("branch"))
			exec1(s.DB, fmt.Sprintf(`CREATE TRIGGER reject_recovery_write BEFORE INSERT ON events
				WHEN NEW.action = '%s' BEGIN SELECT RAISE(ABORT, 'injected recovery write failure'); END`, tc.action))
			f := classifyFailure(job.str("result"), "", "", false)
			line, acted := recoveryWithoutPanic(t, func() (string, bool) {
				return recoverOne(s, p, job, nil, testNow, "daemon", f, job.str("id"), 0, 3)
			})
			if acted || !strings.Contains(line, "injected recovery write failure") {
				t.Errorf("failed write claimed recovery or lost its error: acted=%v line=%q", acted, line)
			}
			after := jobRow(s.DB, job.str("id"))
			for _, key := range []string{"status", "result", "branch", "finished_at", "lease_agent", "brief"} {
				if after.str(key) != job.str(key) {
					t.Errorf("%s changed after rollback: %q -> %q", key, job.str(key), after.str(key))
				}
			}
			if after.int("attempts") != job.int("attempts") {
				t.Errorf("attempts reset after failed recovery: %v", after)
			}
			if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs"); n != 1 {
				t.Errorf("orphan corrected job remained after failed ledger write: %d jobs", n)
			}
			if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action IN ('job.queued', 'job.add', ?)", board.JobRecoverAction); n != 0 {
				t.Errorf("success events survived rollback: %d", n)
			}
			if !git(root, "rev-parse", "--verify", "--quiet", "refs/heads/"+job.str("branch")).ok() {
				t.Error("branch was deleted before recovery writes succeeded")
			}

			exec1(s.DB, "DROP TRIGGER reject_recovery_write")
			line, acted = recoverOne(s, p, job, nil, testNow, "daemon", f, job.str("id"), 0, 3)
			if !acted || line == "" {
				t.Fatalf("recovery did not resume after the write fault cleared: %v %q", acted, line)
			}
			if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = ?", board.JobRecoverAction); n != 1 {
				t.Fatalf("recovery ledger after retry = %d", n)
			}
		})
	}
}

func TestRecoveryResilienceDoesNotMutateStaleFailures(t *testing.T) {
	for _, mode := range []string{"leased", "done", "cancelled", "owner result", "new failure", "active run", "owner question"} {
		t.Run(mode, func(t *testing.T) {
			s := newStore(t)
			p, root := newRepo(t, s)
			job := insertRecoverFailedJob(t, s, "job-stale", "improve", "", "runner error: connection reset", plusMinutes(testNow, -30))
			run(t, root, "git", "branch", job.str("branch"))
			f := classifyFailure(job.str("result"), "", "", false)
			switch mode {
			case "leased", "done", "cancelled":
				exec1(s.DB, "UPDATE jobs SET status = ?, lease_agent = 'other', generation = generation + 1 WHERE id = ?", mode, job.str("id"))
			case "owner result":
				exec1(s.DB, "UPDATE jobs SET result = 'cancelled by the owner: stop retrying' WHERE id = ?", job.str("id"))
			case "new failure":
				exec1(s.DB, "UPDATE jobs SET finished_at = ?, generation = generation + 1 WHERE id = ?", plusMinutes(testNow, -10), job.str("id"))
			case "active run":
				insertRecoverRun(t, s, job.str("id"), "run-active", "running", "", "", job.str("branch"))
			case "owner question":
				_, err := createPost(s, testNow, postOpts{Kind: "question", Project: "proj", Area: "jobs", Thread: job.str("id"),
					Summary: "Owner approval required", Agent: actorAgent(s, "daemon"), ForOwner: true})
				if err != nil {
					t.Fatal(err)
				}
			}
			before := jobRow(s.DB, job.str("id"))
			recoverOne(s, p, job, nil, testNow, "daemon", f, job.str("id"), 0, 3)
			after := jobRow(s.DB, job.str("id"))
			for _, key := range []string{"status", "result", "branch", "finished_at", "lease_agent"} {
				if after.str(key) != before.str(key) {
					t.Errorf("stale recovery changed %s: %q -> %q", key, before.str(key), after.str(key))
				}
			}
			if after.int("attempts") != before.int("attempts") {
				t.Error("stale recovery reset attempts")
			}
			if n := count(t, s, `SELECT COUNT(*) AS n FROM events WHERE action = ?
				AND json_extract(detail, '$.outcome') IN ('requeued', 'replanned')`, board.JobRecoverAction); n != 0 {
				t.Errorf("stale recovery recorded %d successful actions", n)
			}
			if !git(root, "rev-parse", "--verify", "--quiet", "refs/heads/"+job.str("branch")).ok() {
				t.Error("stale recovery deleted the branch")
			}
		})
	}
}

func TestRecoveryResilienceBranchCleanupFailureRemainsRetryable(t *testing.T) {
	s := newStore(t)
	p, root := newRepo(t, s)
	job := insertRecoverFailedJob(t, s, "job-checked-out", "improve", "", "runner error: connection reset", plusMinutes(testNow, -30))
	run(t, root, "git", "checkout", "-q", "-b", job.str("branch"))
	f := classifyFailure(job.str("result"), "", "", false)
	line, acted := recoverOne(s, p, job, nil, testNow, "daemon", f, job.str("id"), 0, 3)
	if acted || !strings.Contains(line, job.str("branch")) {
		t.Fatalf("cleanup failure was not reported: acted=%v line=%q", acted, line)
	}
	if after := jobRow(s.DB, job.str("id")); after.str("status") != "failed" || after.str("branch") != job.str("branch") || after.int("attempts") != 3 {
		t.Fatalf("branch cleanup failure changed retry state: %v", after)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = ?", board.JobRecoverAction); n != 0 {
		t.Fatalf("branch cleanup failure wrote a success event: %d", n)
	}
	run(t, root, "git", "checkout", "-q", "main")
	if line, acted = recoverOne(s, p, job, nil, testNow, "daemon", f, job.str("id"), 0, 3); !acted {
		t.Fatalf("cleanup did not resume after the checkout moved: %q", line)
	}
}

func TestRecoveryResilienceCleanupOnlyDeletesThisJobsBranches(t *testing.T) {
	for _, branch := range []string{"main", "feature/human-work", "pulse/other/job-unrelated"} {
		for _, source := range []string{"job", "run"} {
			t.Run(branch+"/"+source, func(t *testing.T) {
				s := newStore(t)
				p, root := newRepo(t, s)
				job := insertRecoverFailedJob(t, s, "job-cleanup-scope", "improve", "", "runner error: connection reset", plusMinutes(testNow, -30))
				canonical := job.str("branch")
				run(t, root, "git", "branch", canonical)
				if branch != "main" {
					run(t, root, "git", "branch", branch)
				}
				run(t, root, "git", "checkout", "-q", "-b", "unrelated-checkout")
				if source == "job" {
					exec1(s.DB, "UPDATE jobs SET branch = ? WHERE id = ?", branch, job.str("id"))
					job = jobRow(s.DB, job.str("id"))
				} else {
					insertRecoverRun(t, s, job.str("id"), "run-scoped-cleanup", "failed", "runner error", "", branch)
				}
				f := classifyFailure(job.str("result"), "", "", false)
				line, acted := recoverOne(s, p, job, nil, testNow, "daemon", f, job.str("id"), 0, 3)
				if acted || !strings.Contains(line, "not owned by job") {
					t.Errorf("unrelated branch was accepted for cleanup: acted=%v line=%q", acted, line)
				}
				for _, kept := range []string{branch, canonical} {
					if !git(root, "rev-parse", "--verify", "--quiet", "refs/heads/"+kept).ok() {
						t.Errorf("cleanup deleted %s before validating every branch", kept)
					}
				}
				if got := jobRow(s.DB, job.str("id")).str("status"); got != "failed" {
					t.Errorf("unsafe cleanup claimed successful recovery: %s", got)
				}
				if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = ?", board.JobRecoverAction); n != 0 {
					t.Errorf("unsafe cleanup wrote %d success decisions", n)
				}
			})
		}
	}
}

func TestRecoveryResilienceOneCorrectedJobWithoutSource(t *testing.T) {
	s := newStore(t)
	p, _ := newRepo(t, s)
	job := insertRecoverFailedJob(t, s, "job-no-source", "improve", "", "verify exit 1", plusMinutes(testNow, -30))
	f := classifyFailure(job.str("result"), "", "", false)
	var wg sync.WaitGroup
	for range 2 {
		wg.Go(func() {
			recoverOne(s, p, job, nil, testNow, "daemon", f, job.str("id"), 0, 3)
		})
	}
	wg.Wait()
	recoverOne(s, p, job, nil, testNow, "daemon", f, job.str("id"), 0, 3)
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE status = 'queued'"); n != 1 {
		t.Fatalf("one failed job created %d corrected jobs", n)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = ?", board.JobRecoverAction); n != 1 {
		t.Fatalf("one failed job wrote %d recovery events", n)
	}
}

func TestRecoveryResilienceChainHasOneCorrectedCarrier(t *testing.T) {
	s := newStore(t)
	p, _ := newRepo(t, s)
	setRecoverSetting(t, s, "daemon.recover_limit", "5")
	insertRecoverFailedJob(t, s, "job-chain-root", "improve", "", "verify exit 1", plusHours(testNow, -7))
	for _, id := range []string{"job-chain-first", "job-chain-second"} {
		insertRecoverFailedJob(t, s, id, "improve", "", "verify exit 1", plusHours(testNow, -4))
		eventAt(s.DB, plusHours(testNow, -5), "human", board.JobRecoverAction, "job-chain-root",
			map[string]any{"outcome": "replanned", "root": "job-chain-root", "job": id})
	}
	recoverFailedJobs(s, p, testNow, "daemon")
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE status = 'queued'"); n != 1 {
		t.Fatalf("one chain has %d corrected jobs waiting", n)
	}
	carrier := mustOne(s.DB, "SELECT id FROM jobs WHERE status = 'queued'")
	exec1(s.DB, "UPDATE jobs SET status = 'done', finished_at = ? WHERE id = ?", testNow, carrier.str("id"))
	recoverFailedJobs(s, p, plusMinutes(testNow, 1), "daemon")
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE status = 'queued'"); n != 0 {
		t.Fatalf("superseded sibling queued %d copies after the carrier completed", n)
	}
}

func TestRecoveryResilienceWriteErrorsRespectTickCap(t *testing.T) {
	s := newStore(t)
	p, _ := newRepo(t, s)
	for i := range recoverPerTick + 1 {
		insertRecoverFailedJob(t, s, fmt.Sprintf("job-write-cap-%d", i), "improve", "", "runner error", plusHours(testNow, -1))
	}
	exec1(s.DB, `CREATE TRIGGER reject_recovery_write BEFORE INSERT ON events
		WHEN NEW.action = 'job.recover' BEGIN SELECT RAISE(ABORT, 'injected recovery write failure'); END`)
	lines := recoverFailedJobs(s, p, testNow, "daemon")
	if len(lines) != recoverPerTick {
		t.Fatalf("write failures bypassed the tick budget: %d attempts", len(lines))
	}
	if note := recoverMeta(t, s)["proj"].Note; !strings.Contains(note, "cap 5 reached") || !strings.Contains(note, "5 recovery error(s)") {
		t.Fatalf("failed attempts and cap are not visible: %q", note)
	}
}

func TestRecoveryResilienceHeldWriteErrorsRespectTickCap(t *testing.T) {
	for _, mode := range []string{"cancelled", "superseded", "cap finding"} {
		t.Run(mode, func(t *testing.T) {
			s := newStore(t)
			p, _ := newRepo(t, s)
			for i := range recoverPerTick + 1 {
				id := fmt.Sprintf("job-held-write-%d", i)
				result := "verify exit 1"
				if mode == "cancelled" {
					result = "cancelled by the owner"
				}
				job := insertRecoverFailedJob(t, s, id, "improve", "test:"+id, result, plusHours(testNow, -1))
				switch mode {
				case "superseded":
					copy, _, err := addJob(s, "proj", "improve", "New carrier", "brief", 0, 0, job.str("source"), "daemon", true)
					if err != nil {
						t.Fatal(err)
					}
					if _, err := finishJob(s, copy.str("id"), "daemon", "done", "already repaired", "", -1); err != nil {
						t.Fatal(err)
					}
				case "cap finding":
					recoveryEvent(s.DB, testNow, "daemon", id, "held",
						failure{Class: failureDeterministic, Reason: "recovered 3 times; the lead and the owner decide"}, id, 3, "")
				}
			}
			if mode == "cap finding" {
				exec1(s.DB, `CREATE TRIGGER reject_held_write BEFORE INSERT ON posts
					WHEN NEW.kind = 'finding' BEGIN SELECT RAISE(ABORT, 'injected held write failure'); END`)
			} else {
				exec1(s.DB, `CREATE TRIGGER reject_held_write BEFORE INSERT ON events
					WHEN NEW.action = 'job.recover' BEGIN SELECT RAISE(ABORT, 'injected held write failure'); END`)
			}
			lines := recoverFailedJobs(s, p, testNow, "daemon")
			if len(lines) != recoverPerTick {
				t.Fatalf("held write failures bypassed recovery budget: %d attempts, want %d", len(lines), recoverPerTick)
			}
			if note := recoverMeta(t, s)["proj"].Note; !strings.Contains(note, "cap 5 reached") || !strings.Contains(note, "5 recovery error(s)") {
				t.Fatalf("held write failures were not surfaced with their budget: %q", note)
			}
			if mode == "cap finding" {
				exec1(s.DB, "DROP TRIGGER reject_held_write")
				for i, want := range []int{recoverPerTick, recoverPerTick + 1} {
					recoverFailedJobs(s, p, plusMinutes(testNow, float64(i+1)), "daemon")
					if n := count(t, s, "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding'"); n != want {
						t.Fatalf("notification repair exceeded its cap or was starved: %d findings, want %d", n, want)
					}
				}
			}
		})
	}
}

func TestRecoveryResilienceSameSecondFailureIsNotStranded(t *testing.T) {
	s := newStore(t)
	p, _ := newRepo(t, s)
	job := insertRecoverFailedJob(t, s, "job-fast-failure", "improve", "", "runner error", plusMinutes(testNow, -30))
	recoverFailedJobs(s, p, testNow, "daemon")
	leased, err := leaseJob(s, "w1", testNow, leaseOpts{JobID: job.str("id")})
	if err != nil || leased == nil {
		t.Fatalf("lease recovered job: %v %v", leased, err)
	}
	if _, err := finishJob(s, job.str("id"), "w1", "failed", "runner error: connection reset", "", leased.int("generation")); err != nil {
		t.Fatal(err)
	}
	if !recoveryPending(s, job.str("id")) {
		t.Fatal("new failure was mistaken for the recovery recorded in the same second")
	}
	recoverFailedJobs(s, p, plusMinutes(testNow, 16), "daemon")
	if got := jobRow(s.DB, job.str("id")).str("status"); got != "queued" {
		t.Fatalf("new failure did not recover after its next backoff: %s", got)
	}
	if detail := recoverDetail(t, s, job.str("id")); detail["n"].(float64) != 2 {
		t.Fatalf("same-second retry lost its chain budget: %v", detail)
	}
}

func TestRecoveryResiliencePendingMatchesRecoveryGates(t *testing.T) {
	for _, mode := range []string{"answer", "lead", "baseline", "experiment", "old", "cancelled", "lead followup", "superseded", "active run"} {
		t.Run(mode, func(t *testing.T) {
			s := newStore(t)
			newRepo(t, s)
			job := insertRecoverFailedJob(t, s, "job-not-pending", "task", "task:TASK-guard", "runner error: connection reset", plusHours(testNow, -1))
			switch mode {
			case "answer", "lead", "baseline", "experiment":
				exec1(s.DB, "UPDATE jobs SET kind = ? WHERE id = ?", mode, job.str("id"))
			case "old":
				exec1(s.DB, "UPDATE jobs SET finished_at = ? WHERE id = ?", plusHours(testNow, -24*8), job.str("id"))
			case "cancelled":
				exec1(s.DB, "UPDATE jobs SET result = 'cancelled by the owner: runner error' WHERE id = ?", job.str("id"))
			case "lead followup":
				eventAt(s.DB, testNow, "daemon", followUpAction, "TASK-guard", map[string]any{"job": "job-lead"})
			case "superseded":
				if _, _, err := addJob(s, "proj", "task", "New carrier", "brief", 1, 5, job.str("source"), "daemon", true); err != nil {
					t.Fatal(err)
				}
			case "active run":
				insertRecoverRun(t, s, job.str("id"), "run-active", "running", "", "", "")
			}
			if recoveryPending(s, job.str("id")) {
				t.Fatal("lead would wait for a recovery the daemon will not perform")
			}
		})
	}
}

func TestRecoveryResilienceCorrectedBriefIsBounded(t *testing.T) {
	s := newStore(t)
	_, root := newRepo(t, s)
	logPath := filepath.Join(root, "recovery.log")
	if err := os.WriteFile(logPath, []byte(strings.Repeat("prior debug output\n", 500)+"last diagnostic detail\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	original := "Fix the parser. Preserve the owner's protected files and use the existing test fixture."
	brief := original
	retryNote := "\n\n## Retry note (human)\n\nUse the approved parser boundary; do not edit fixtures."
	for i := range 12 {
		job := row{"id": fmt.Sprintf("job-chain-%d", i), "brief": brief, "finished_at": testNow}
		run := row{"id": fmt.Sprintf("run-chain-%d", i), "executor": "local", "log": logPath, "summary": "verify failed"}
		f := classifyFailure(fmt.Sprintf("verify exit 1: attempt %d tried a different parser guard", i), "", "", false)
		brief = correctedBrief(job, run, f)
		if i == 4 {
			brief += retryNote
		}
	}
	for _, want := range []string{
		original, retryNote, "job-chain-11", "run-chain-11", "attempt 11", "last diagnostic detail",
		"ask the current peer or lead once", "attempted fixes", "SUMMARY: blocked:", "SUMMARY: failed:", "QUESTION: only",
	} {
		if !strings.Contains(brief, want) {
			t.Errorf("corrected brief lost %q:\n%s", want, brief)
		}
	}
	if n := strings.Count(brief, "## Earlier attempt (ADR-036)"); n != 1 {
		t.Errorf("brief recursively retained %d recovery sections", n)
	}
	if n := strings.Count(brief, "Last lines of its run log:"); n != 1 {
		t.Errorf("brief retained %d full old log tails", n)
	}
	if len(brief) > len(original)+len(retryNote)+8000 {
		t.Errorf("recovery context grew beyond its bounded evidence budget: %d bytes", len(brief))
	}
}

func TestRecoveryResilienceCompactsLegacyBriefWithoutDroppingInstructions(t *testing.T) {
	original := "Fix the parser.\n\n## Earlier attempt (ADR-036)\n\nJob `example` is documentation, not a generated failure.\n" + recoveryBriefEnd
	legacy := original + recoveryBriefHeading +
		"Job `job-old` (run `run-old`, executor `local`) failed at " + testNow + ": verify exit 1.\n\n" +
		"Last lines of its run log:\n\n```\nold log\n```\n\n" +
		"First look at the default branch: when the work above already landed another way, stop and say so in SUMMARY:. " +
		"Otherwise start from the cause: reproduce the failing check, fix it, then deliver the original task above. " +
		recoveryBriefEnd + "\n\n## Retry note (human)\n\nKeep the fixture unchanged."
	job := row{"id": "job-current", "brief": legacy, "finished_at": testNow}
	brief := correctedBrief(job, nil, classifyFailure("verify exit 1", "", "", false))
	for _, want := range []string{original, "job-old", "job-current", "## Retry note (human)\n\nKeep the fixture unchanged."} {
		if !strings.Contains(brief, want) {
			t.Errorf("compaction lost original instructions or evidence %q:\n%s", want, brief)
		}
	}
	if strings.Contains(brief, "old log") {
		t.Error("legacy log was recursively copied into the corrected brief")
	}
}

func TestRecoveryResilienceMissingLogIsExplicit(t *testing.T) {
	for _, mode := range []string{"no run", "no path", "missing", "directory"} {
		t.Run(mode, func(t *testing.T) {
			root := t.TempDir()
			job := row{"id": "job-log", "brief": "Fix the parser.", "finished_at": testNow}
			run := row{"id": "run-log", "executor": "local", "summary": "recorded summary"}
			switch mode {
			case "no run":
				run = nil
			case "missing":
				run["log"] = filepath.Join(root, "missing.log")
			case "directory":
				run["log"] = root
			}
			f := classifyFailure("worker exited 1 github_pat_abcdefghijklmnopqrstuvwxyz", "", "", false)
			brief := correctedBrief(job, run, f)
			if !strings.Contains(strings.ToLower(brief), "unavailable") {
				t.Errorf("missing failure evidence was silently dropped:\n%s", brief)
			}
			if run != nil && !strings.Contains(brief, "recorded summary") {
				t.Errorf("recorded summary was lost:\n%s", brief)
			}
			if strings.Contains(brief, "github_pat_abcdefghijklmnopqrstuvwxyz") {
				t.Error("failure reason leaked a token-shaped value")
			}
		})
	}
}

func TestRecoveryResilienceCapFindingRetriesAfterWriteFailure(t *testing.T) {
	s := newStore(t)
	p, _ := newRepo(t, s)
	setRecoverSetting(t, s, "daemon.recover_limit", "1")
	job := insertRecoverFailedJob(t, s, "job-cap-finding", "improve", "", "verify exit 1: parser still rejects empty input", plusMinutes(testNow, -30))
	eventAt(s.DB, plusHours(testNow, -2), "daemon", board.JobRecoverAction, job.str("id"),
		map[string]any{"outcome": "requeued", "root": job.str("id"), "n": 1})
	exec1(s.DB, `CREATE TRIGGER reject_cap_finding BEFORE INSERT ON posts
		WHEN NEW.kind = 'finding' BEGIN SELECT RAISE(ABORT, 'injected finding write failure'); END`)
	lines := recoverFailedJobs(s, p, testNow, "daemon")
	if !strings.Contains(strings.Join(lines, "\n"), "injected finding write failure") {
		t.Errorf("capped finding write failure was hidden: %v", lines)
	}
	exec1(s.DB, "DROP TRIGGER reject_cap_finding")
	recoverFailedJobs(s, p, plusMinutes(testNow, 1), "daemon")
	recoverFailedJobs(s, p, plusMinutes(testNow, 2), "daemon")
	if n := count(t, s, "SELECT COUNT(*) AS n FROM posts WHERE kind = 'finding' AND thread = ?", job.str("id")); n != 1 {
		t.Fatalf("cap finding was stranded or duplicated: %d", n)
	}
	post := mustOne(s.DB, "SELECT body FROM posts WHERE kind = 'finding' AND thread = ?", job.str("id"))
	for _, want := range []string{"parser still rejects empty input", "pulse job retry " + job.str("id") + " --as-new"} {
		if !strings.Contains(post.str("body"), want) {
			t.Errorf("capped finding lacks actionable evidence %q: %s", want, post.str("body"))
		}
	}
}
