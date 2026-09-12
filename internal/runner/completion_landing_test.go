package runner

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
)

func TestCompletionPendingWaitsForController(t *testing.T) {
	for _, kind := range []string{"task", "experiment"} {
		t.Run(kind, func(t *testing.T) {
			s, job, project, agent := newCompletionJob(t, kind)
			branch := "pulse/" + agent + "/" + job.str("id")
			if kind == "experiment" {
				branch = "evolve/completion-fixture"
			}
			exec1(s.DB, "INSERT INTO runs (id, job_id, agent, project, started_at, status) VALUES ('run-completion', ?, ?, 'proj', ?, 'running')",
				job.str("id"), agent, testNow)
			if _, err := finishJob(s, job.str("id"), agent, "done", "worker finished before collection", branch, -1); err != nil {
				t.Fatal(err)
			}
			if got := pendingLandings(s.DB, "proj"); len(got) != 0 {
				t.Errorf("worker-reported done is eligible while controller runs: %d job(s)", len(got))
			}
			if kind == "task" {
				if got := landPending(s, project, "daemon", testNow, true, 0, true); len(got) != 0 {
					t.Errorf("active controller reached landing: %+v", got)
				}
				if n := count(t, s, "SELECT COUNT(*) AS n FROM events WHERE action = 'land.missing'"); n != 0 {
					t.Errorf("active controller emitted %d land.missing event(s)", n)
				}
				if got := jobRow(s.DB, job.str("id")).str("branch"); got != branch {
					t.Errorf("active controller lost provisional branch: %q", got)
				}
			}
		})
	}
}

func TestCompletionPendingLegacyFinishedJobs(t *testing.T) {
	for _, priorRun := range []string{"", "done", "failed", "lost"} {
		t.Run("prior-"+priorRun, func(t *testing.T) {
			s, job, _, agent := newCompletionJob(t, "task")
			branch := "pulse/" + agent + "/" + job.str("id")
			if priorRun != "" {
				exec1(s.DB, "INSERT INTO runs (id, job_id, agent, project, started_at, status) VALUES ('run-legacy', ?, ?, 'proj', ?, ?)",
					job.str("id"), agent, testNow, priorRun)
			}
			if _, err := finishJob(s, job.str("id"), humanAgent, "done", "finished by hand", branch, -1); err != nil {
				t.Fatal(err)
			}
			if _, err := finishJob(s, job.str("id"), humanAgent, "done", "updated the result", "", -1); err != nil {
				t.Fatal(err)
			}
			pending := pendingLandings(s.DB, "proj")
			if len(pending) != 1 || pending[0].str("branch") != branch {
				t.Fatalf("manual finish lost eligibility: %v", pending)
			}
		})
	}
}

func TestCompletionFinalizationDoesNotExposeProvisionalBranch(t *testing.T) {
	s, job, _, agent := newCompletionJob(t, leadKind)
	exec1(s.DB, "CREATE TABLE completion_snapshot (status TEXT, branch TEXT)")
	exec1(s.DB, `CREATE TRIGGER completion_snapshot AFTER UPDATE OF status ON runs
		WHEN NEW.status != 'running' BEGIN
		INSERT INTO completion_snapshot SELECT status, branch FROM jobs WHERE id = NEW.job_id;
		END`)
	command := completionWorkerCommand(t, "done", "SUMMARY: reviewed the queue", "", 0, "")
	runCompletion(t, s, job, agent, command)
	snapshots := mustRows(s.DB, "SELECT * FROM completion_snapshot")
	if len(snapshots) != 1 || snapshots[0].str("status") != "done" || snapshots[0].str("branch") != "" {
		t.Fatalf("run finalization exposed provisional job state: %v", snapshots)
	}
}

func TestCompletionVerifiedCommitPendingAfterFinalization(t *testing.T) {
	s, job, project, agent := newCompletionJob(t, "task")
	leased, err := leaseJob(s, agent, testNow, leaseOpts{JobID: job.str("id")})
	if err != nil || leased == nil {
		t.Fatalf("lease: %v", err)
	}
	gen := leased.int("generation")
	exec1(s.DB, "INSERT INTO runs (id, job_id, agent, project, started_at, status) VALUES ('run-commit', ?, ?, 'proj', ?, 'running')",
		job.str("id"), agent, testNow)
	if _, err := finishJob(s, job.str("id"), agent, "done", "worker report", "pulse/aurora/provisional", gen); err != nil {
		t.Fatal(err)
	}
	root := project.str("path")
	base := strings.TrimSpace(run(t, root, "git", "rev-parse", "HEAD"))
	branch := "pulse/" + agent + "/" + job.str("id")
	run(t, root, "git", "switch", "-q", "-c", branch)
	key := filepath.Join(t.TempDir(), "completion.key")
	pub, err := makeRunKey(key, agent, "run-commit")
	if err != nil {
		t.Fatal(err)
	}
	if err := configureCloneIdentity(root, agent, key, provenanceHook(agent, job.str("id"), localExecutor)); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(root, "completion.txt"), []byte("verified fixture\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if _, err := gitCheck(root, "add", "completion.txt"); err != nil {
		t.Fatal(err)
	}
	if _, err := gitCheck(root, "commit", "-q", "-m", "Complete the fixture"); err != nil {
		t.Fatal(err)
	}
	if _, err := gitCheck(root, "diff", "--check", base+".."+branch); err != nil {
		t.Fatal(err)
	}
	if problems := landingProblems(root, base, branch, agent, job.str("id"), pub, filepath.Join(t.TempDir(), "signers")); len(problems) != 0 {
		t.Fatalf("fixture commit did not verify: %v", problems)
	}
	got, err := board.FinishRunJob(s, board.RunJobResult{RunID: "run-commit", JobID: job.str("id"), Agent: agent,
		Status: "done", Result: "verified fixture", Branch: branch, Generation: gen})
	if err != nil || got.Branch != branch {
		t.Fatalf("verified branch not retained: %+v %v", got, err)
	}
	if pending := pendingLandings(s.DB, "proj"); len(pending) != 0 {
		t.Fatalf("job became eligible before its run finalized: %v", pending)
	}
	exec1(s.DB, "UPDATE runs SET status = 'done', finished_at = ?, branch = ?, commits = 1, verify = 'verify exit 0' WHERE id = 'run-commit'",
		testNow, branch)
	pending := pendingLandings(s.DB, "proj")
	if len(pending) != 1 || pending[0].str("id") != job.str("id") || pending[0].str("branch") != branch {
		t.Fatalf("verified branch not eligible after finalization: %v", pending)
	}
	if exists, why := branchExists(root, branch); !exists || why != "" {
		t.Fatalf("eligible artifact does not exist: %v %q", exists, why)
	}
}

func TestCompletionDryRunKeepsPlannedExperimentBranch(t *testing.T) {
	s, job, _, agent := newCompletionJob(t, "experiment")
	exec1(s.DB, "UPDATE jobs SET branch = 'evolve/completion-fixture' WHERE id = ?", job.str("id"))
	r, reason, err := workOnce(s, agent, "proj", job.str("id"), testNow, runOpts{DryRun: true})
	if err != nil || r == nil || r.str("status") != "dry-run" {
		t.Fatalf("dry run: %v %q %v", r, reason, err)
	}
	j := jobRow(s.DB, job.str("id"))
	if j.str("status") != "queued" || j.str("branch") != "evolve/completion-fixture" || len(pendingLandings(s.DB, "proj")) != 0 {
		t.Fatalf("dry run lost the planned experiment branch: %v", j)
	}
}
