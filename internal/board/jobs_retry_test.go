package board

import (
	"encoding/json"
	"strings"
	"sync"
	"testing"

	"github.com/manishtiwari25/pulse/internal/cli"
	"github.com/manishtiwari25/pulse/internal/store"
)

func makeFailedJob(t *testing.T, s *store.Store, project, source string) *Job {
	t.Helper()
	job, _, err := AddJob(s, JobInput{Project: project, Kind: "task", Title: "Fix the failing check",
		Brief: "Original brief", Tier: 1, Priority: 7, Source: source, CreatedBy: "alice"})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := FinishJob(s, job.ID, "alice", "failed", strings.Repeat("x", 220), "pulse/alice/"+job.ID, nil); err != nil {
		t.Fatal(err)
	}
	if _, err := s.DB.Exec("UPDATE jobs SET attempts = 4, lease_agent = 'alice', lease_until = '2026-09-06T11:00:00Z' WHERE id = ?", job.ID); err != nil {
		t.Fatal(err)
	}
	job, err = JobByID(s, job.ID)
	if err != nil {
		t.Fatal(err)
	}
	return job
}

func recoverEvent(t *testing.T, s *store.Store, jobID string) map[string]any {
	t.Helper()
	var detail string
	if err := s.DB.QueryRow("SELECT detail FROM events WHERE action = ? AND subject = ? ORDER BY seq DESC LIMIT 1", JobRecoverAction, jobID).Scan(&detail); err != nil {
		t.Fatal(err)
	}
	var doc map[string]any
	if err := json.Unmarshal([]byte(detail), &doc); err != nil {
		t.Fatalf("recover detail is not JSON: %v\n%s", err, detail)
	}
	return doc
}

func TestRetryJobInPlaceQueuesFailedJob(t *testing.T) {
	_, s, p := fixture(t)
	job := makeFailedJob(t, s, p.Slug, "task:one")

	result, err := RetryJob(s, RetryInput{JobID: job.ID, Brief: "Run the check first.", Actor: "human"})
	if err != nil {
		t.Fatal(err)
	}
	if result.Outcome != "retried" || result.Root != job.ID || result.Job.ID != job.ID {
		t.Fatalf("result %+v", result)
	}
	got := result.Job
	if got.Status != "queued" || got.Attempts != 0 || got.Branch != "" || got.FinishedAt != "" || got.LeaseAgent != "" || got.LeaseUntil != "" {
		t.Fatalf("job was not reset: %+v", got)
	}
	if !strings.HasPrefix(got.Result, "retried by human: ") || len([]rune(strings.TrimPrefix(got.Result, "retried by human: "))) != 200 {
		t.Fatalf("result was not rewritten with truncated cause: %q", got.Result)
	}
	if !strings.Contains(got.Brief, "## Retry note (human, "+testNow+")\n\nRun the check first.") {
		t.Fatalf("brief was not appended:\n%s", got.Brief)
	}
	if n := countRows(t, s, "events", "action = 'job.queued' AND subject = ?", job.ID); n != 1 {
		t.Fatalf("%d job.queued events", n)
	}
	detail := recoverEvent(t, s, job.ID)
	if detail["outcome"] != "retried" || detail["root"] != job.ID || detail["class"] != "human" || detail["why"] != "pulse job retry by human" || detail["signature"] != "" || detail["n"].(float64) != 1 {
		t.Fatalf("detail %+v", detail)
	}
}

func TestRetryJobKeepsRecoverRootAndIncrements(t *testing.T) {
	_, s, p := fixture(t)
	root := "job-root"
	job := makeFailedJob(t, s, p.Slug, "task:chain")
	if err := event(s, "daemon", JobRecoverAction, root, map[string]any{"outcome": "replanned", "root": root, "job": job.ID}); err != nil {
		t.Fatal(err)
	}

	result, err := RetryJob(s, RetryInput{JobID: job.ID, Actor: "human"})
	if err != nil {
		t.Fatal(err)
	}
	if result.Root != root || result.n != 2 {
		t.Fatalf("chain result %+v", result)
	}
	detail := recoverEvent(t, s, job.ID)
	if detail["root"] != root || detail["n"].(float64) != 2 {
		t.Fatalf("detail %+v", detail)
	}
}

func TestRetryJobAsNewQueuesOneCorrectedCopy(t *testing.T) {
	_, s, p := fixture(t)
	job := makeFailedJob(t, s, p.Slug, "task:copy")

	result, err := RetryJob(s, RetryInput{JobID: job.ID, AsNew: true, Brief: "Correct the test fixture.", Actor: "human"})
	if err != nil {
		t.Fatal(err)
	}
	copyJob := result.Job
	if result.Outcome != "replanned" || result.Root != job.ID || copyJob.ID == job.ID || copyJob.Source != job.Source {
		t.Fatalf("result %+v copy %+v", result, copyJob)
	}
	if copyJob.Project != job.Project || copyJob.Kind != job.Kind || copyJob.Tier != job.Tier || copyJob.Priority != job.Priority || copyJob.Status != "queued" {
		t.Fatalf("copy did not keep job shape: %+v", copyJob)
	}
	if !strings.HasPrefix(copyJob.Title, "Retry "+job.ID+": Fix the failing check") || len([]rune(copyJob.Title)) > 200 {
		t.Fatalf("copy title %q", copyJob.Title)
	}
	for _, want := range []string{"Original brief", "## Earlier attempt (ADR-036)", "Job `" + job.ID + "` failed at " + testNow + ": ", "Correct the test fixture.", "Start from the cause, fix it, then deliver the original task above."} {
		if !strings.Contains(copyJob.Brief, want) {
			t.Fatalf("copy brief missing %q:\n%s", want, copyJob.Brief)
		}
	}
	detail := recoverEvent(t, s, job.ID)
	if detail["outcome"] != "replanned" || detail["root"] != job.ID || detail["job"] != copyJob.ID || detail["n"].(float64) != 1 {
		t.Fatalf("detail %+v", detail)
	}

	again, err := RetryJob(s, RetryInput{JobID: job.ID, AsNew: true, Actor: "human"})
	if err != nil {
		t.Fatal(err)
	}
	if again.Job.ID != copyJob.ID || !again.reused {
		t.Fatalf("second retry did not reuse the open copy: %+v", again)
	}
	if n := countRows(t, s, "jobs", "source = ? AND status = 'queued'", job.Source); n != 1 {
		t.Fatalf("%d corrected jobs", n)
	}
}

func TestRetryJobRefusalsAndCancelled(t *testing.T) {
	_, s, p := fixture(t)
	if _, err := RetryJob(s, RetryInput{JobID: "missing", Actor: "human"}); what(err) != "No job 'missing'" {
		t.Fatalf("missing: %v", err)
	}
	done := makeFailedJob(t, s, p.Slug, "task:done")
	if _, err := FinishJob(s, done.ID, "human", "done", "ok", "", nil); err != nil {
		t.Fatal(err)
	}
	if _, err := RetryJob(s, RetryInput{JobID: done.ID, Actor: "human"}); !strings.Contains(what(err), "nothing to retry") {
		t.Fatalf("done: %v", err)
	}
	queued, _, err := AddJob(s, JobInput{Project: p.Slug, Kind: "task", Title: "Queued", Tier: 1, CreatedBy: "alice"})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := RetryJob(s, RetryInput{JobID: queued.ID, Actor: "human"}); !strings.Contains(what(err), "already running") {
		t.Fatalf("queued: %v", err)
	}
	leased, _, err := AddJob(s, JobInput{Project: p.Slug, Kind: "task", Title: "Leased", Tier: 1, CreatedBy: "alice"})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.DB.Exec("UPDATE jobs SET status = 'leased', lease_agent = 'alice' WHERE id = ?", leased.ID); err != nil {
		t.Fatal(err)
	}
	if _, err := RetryJob(s, RetryInput{JobID: leased.ID, Actor: "human"}); !strings.Contains(what(err), "already running") {
		t.Fatalf("leased: %v", err)
	}
	cancelled := makeFailedJob(t, s, p.Slug, "task:cancelled")
	if _, err := FinishJob(s, cancelled.ID, "human", "cancelled", "stopped", "", nil); err != nil {
		t.Fatal(err)
	}
	if got, err := RetryJob(s, RetryInput{JobID: cancelled.ID, Actor: "human"}); err != nil || got.Job.Status != "queued" {
		t.Fatalf("cancelled retry: %+v %v", got, err)
	}
}

func TestRetryCommandOutputAndJSON(t *testing.T) {
	_, s, p := fixture(t)
	job := makeFailedJob(t, s, p.Slug, "task:cli")
	out, err := runCmd(t, s, &cli.Context{Agent: "alice"}, "job", "retry", job.ID, "--brief", "again")
	if err != nil || strings.TrimSpace(out) != "job "+job.ID+" queued again (retry 1 of this chain)" {
		t.Fatalf("retry output: %q %v", out, err)
	}

	job = makeFailedJob(t, s, p.Slug, "task:cli-copy")
	out, err = runCmd(t, s, &cli.Context{Agent: "alice"}, "job", "retry", job.ID, "--as-new")
	if err != nil || !strings.HasPrefix(strings.TrimSpace(out), "job "+job.ID+" re-planned as job-") {
		t.Fatalf("as-new output: %q %v", out, err)
	}
	out, err = runCmd(t, s, &cli.Context{Agent: "alice"}, "job", "retry", job.ID, "--as-new")
	if err != nil || !strings.Contains(out, "already has open corrected job") {
		t.Fatalf("as-new reuse output: %q %v", out, err)
	}

	job = makeFailedJob(t, s, p.Slug, "task:json")
	out, err = runCmd(t, s, &cli.Context{Agent: "alice", JSON: true}, "job", "retry", job.ID)
	if err != nil {
		t.Fatal(err)
	}
	var result RetryResult
	if err := json.Unmarshal([]byte(out), &result); err != nil {
		t.Fatal(err)
	}
	if result.Outcome != "retried" || result.Root != job.ID || result.Job.ID != job.ID {
		t.Fatalf("json result %+v", result)
	}
}

func TestRetryJobNoSourceAndSupersededInPlace(t *testing.T) {
	_, s, p := fixture(t)
	job := makeFailedJob(t, s, p.Slug, "")
	var wg sync.WaitGroup
	for range 2 {
		wg.Go(func() {
			if _, err := RetryJob(s, RetryInput{JobID: job.ID, AsNew: true}); err != nil {
				t.Error(err)
			}
		})
	}
	wg.Wait()
	if n := countRows(t, s, "jobs", "status = 'queued'"); n != 1 {
		t.Fatalf("empty-source retries made %d queued copies", n)
	}
	again, err := RetryJob(s, RetryInput{JobID: job.ID})
	if err != nil || !again.reused || again.Job.ID == job.ID {
		t.Fatalf("in-place retry revived superseded failure: %+v %v", again, err)
	}
	if n := countRows(t, s, "events", "action = ? AND subject = ?", JobRecoverAction, job.ID); n != 1 {
		t.Fatalf("reusing a carrier consumed the recovery cap: %d events", n)
	}
	if _, err := FinishJob(s, again.Job.ID, "human", "done", "landed", "", nil); err != nil {
		t.Fatal(err)
	}
	for _, asNew := range []bool{false, true} {
		if _, err := RetryJob(s, RetryInput{JobID: job.ID, AsNew: asNew}); err == nil || !strings.Contains(what(err), "superseded") {
			t.Fatalf("succeeded lineage retried: %v", err)
		}
	}
}

func TestRetryJobActiveRunRefused(t *testing.T) {
	_, s, p := fixture(t)
	job := makeFailedJob(t, s, p.Slug, "")
	if _, err := s.DB.Exec(`INSERT INTO runs(id,job_id,agent,project,started_at,status)
		VALUES ('run-active',?,'alice',?,?,'running')`, job.ID, p.Slug, testNow); err != nil {
		t.Fatal(err)
	}
	for _, asNew := range []bool{false, true} {
		if _, err := RetryJob(s, RetryInput{JobID: job.ID, AsNew: asNew}); err == nil || !strings.Contains(what(err), "active run") {
			t.Fatalf("provisional failure retried: %v", err)
		}
	}
}

func TestRetryJobSourceDedupeStaysInProject(t *testing.T) {
	_, s, p := fixture(t)
	job := makeFailedJob(t, s, p.Slug, "shared-source")
	if _, err := s.DB.Exec("INSERT INTO projects(slug,name,path,added_at) VALUES ('other','Other',?,?)", t.TempDir(), testNow); err != nil {
		t.Fatal(err)
	}
	if _, _, err := AddJob(s, JobInput{Project: "other", Kind: "task", Title: "Other work", Tier: 1, Source: job.Source}); err != nil {
		t.Fatal(err)
	}
	out, err := RetryJob(s, RetryInput{JobID: job.ID, AsNew: true})
	if err != nil || out.Job.Project != p.Slug || out.reused {
		t.Fatalf("retry returned another project's work: %+v %v", out, err)
	}
}

func retryStoreAt(t *testing.T, s *store.Store, hours float64) *store.Store {
	t.Helper()
	now, err := PlusHours(testNow, hours)
	if err != nil {
		t.Fatal(err)
	}
	later, err := store.Open(store.Options{Home: s.Home, Now: now, SkipRecords: true})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = later.Close() })
	return later
}

func TestRetryJobCancelledRootAllowsLaterChildRecovery(t *testing.T) {
	for _, source := range []string{"", "manual:cancelled-chain"} {
		for _, asNew := range []bool{false, true} {
			t.Run(source+"/"+map[bool]string{false: "in place", true: "as new"}[asNew], func(t *testing.T) {
				_, s, p := fixture(t)
				root := makeFailedJob(t, s, p.Slug, source)
				cancelled, err := FinishJob(s, root.ID, store.HumanAgent, "cancelled", "cancelled by the owner", "", nil)
				if err != nil {
					t.Fatal(err)
				}
				child, err := RetryJob(retryStoreAt(t, s, 1), RetryInput{JobID: root.ID, AsNew: true, Actor: store.HumanAgent})
				if err != nil {
					t.Fatal(err)
				}
				later := retryStoreAt(t, s, 2)
				if _, err := FinishJob(later, child.Job.ID, store.HumanAgent, "failed", "verify exit 1", "", nil); err != nil {
					t.Fatal(err)
				}
				retry, err := RetryJob(later, RetryInput{JobID: child.Job.ID, AsNew: asNew, Actor: store.HumanAgent})
				if err != nil {
					t.Fatalf("older root cancellation blocked the later authorized child: %v", err)
				}
				if retry.Job.Status != "queued" || retry.Root != root.ID {
					t.Fatalf("child recovery = %+v", retry)
				}
				rootAfter, err := JobByID(s, root.ID)
				if err != nil || rootAfter.Status != "cancelled" || rootAfter.FinishedAt != cancelled.FinishedAt || rootAfter.Result != cancelled.Result {
					t.Fatalf("cancelled history was changed: %+v %v", rootAfter, err)
				}
			})
		}
	}
}
