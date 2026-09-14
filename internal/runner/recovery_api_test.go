package runner

import (
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"sync"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/store"
)

func repairFixture(t *testing.T) (*store.Store, string) {
	t.Helper()
	s := newStore(t)
	_, root := newRepo(t, s)
	return s, root
}

func repairFailure(t *testing.T, s *store.Store, id, result string) row {
	t.Helper()
	insertRecoverFailedJob(t, s, id, "improve", "", result, plusMinutes(testNow, -30))
	exec1(s.DB, "UPDATE jobs SET branch = '', lease_agent = '' WHERE id = ?", id)
	return jobRow(s.DB, id)
}

func repairOne(t *testing.T, s *store.Store, id string) RepairJobResult {
	t.Helper()
	out, err := RepairFailedJobs(s, RepairJobsInput{Project: "proj", JobIDs: []string{id}})
	if err != nil || len(out.Results) != 1 {
		t.Fatalf("repair: %+v %v", out, err)
	}
	return out.Results[0]
}

func TestJobRepairClassificationAndGates(t *testing.T) {
	for _, tc := range []struct {
		name, result, category, class, outcome string
	}{
		{"deterministic", "verify exit 1", "eligible", "deterministic", "replanned"},
		{"transient", "runner error: connection reset", "eligible", "transient", "requeued"},
		{"missing tool", "verify exit 127", "structural", "structural", "skipped"},
		{"hard stop", "needs a real decision", "structural", "structural", "skipped"},
		{"cancelled result", "cancelled by the owner: stop", "cancelled", "", "skipped"},
		{"cancelled summary", "runner error", "cancelled", "", "skipped"},
		{"cancelled status", "verify exit 1", "cancelled", "", "skipped"},
		{"active run", "verify exit 1", "active_run", "", "skipped"},
		{"inactive", "verify exit 1", "inactive_project", "", "skipped"},
		{"stop", "verify exit 1", "stopped", "", "skipped"},
		{"project stop", "verify exit 1", "stopped", "", "skipped"},
		{"project dangling stop", "verify exit 1", "stopped", "", "skipped"},
		{"brain stop", "verify exit 1", "stopped", "", "skipped"},
		{"recovery off", "verify exit 1", "recovery_off", "", "skipped"},
		{"cap", "verify exit 1", "recovery_cap", "", "skipped"},
		{"backoff", "verify exit 1", "backoff", "", "skipped"},
		{"history", "verify exit 1", "outside_window", "", "skipped"},
		{"bad timestamp", "verify exit 1", "structural", "", "skipped"},
		{"tier 2", "verify exit 1", "structural", "structural", "skipped"},
		{"lead", "verify exit 1", "manual_controller", "", "skipped"},
		{"baseline", "verify exit 1", "manual_controller", "", "skipped"},
		{"experiment", "verify exit 1", "manual_controller", "", "skipped"},
		{"answer", "verify exit 1", "manual_controller", "", "skipped"},
		{"question", "verify exit 1", "structural", "structural", "skipped"},
		{"held", "verify exit 1", "held", "", "skipped"},
		{"task deterministic", "verify exit 1", "manual_controller", "", "skipped"},
		{"task transient", "connection reset", "eligible", "transient", "requeued"},
		{"task followed up", "connection reset", "manual_controller", "", "skipped"},
		{"task missing", "connection reset", "manual_controller", "", "skipped"},
		{"task cancelled", "connection reset", "cancelled", "", "skipped"},
		{"task done", "connection reset", "superseded", "", "skipped"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s, root := repairFixture(t)
			id := "job-repair"
			repairFailure(t, s, id, tc.result)
			switch tc.name {
			case "cancelled summary":
				insertRecoverRun(t, s, id, "run-repair", "failed", "cancelled by the owner", "", "")
			case "cancelled status":
				exec1(s.DB, "UPDATE jobs SET status = 'cancelled' WHERE id = ?", id)
			case "active run":
				insertRecoverRun(t, s, id, "run-repair", "running", "", "", "")
			case "inactive":
				exec1(s.DB, "UPDATE projects SET status = 'inactive' WHERE slug = 'proj'")
			case "stop":
				setRecoverSetting(t, s, "stop", "1")
			case "project stop", "project dangling stop", "brain stop":
				if tc.name == "brain stop" {
					root = t.TempDir()
					exec1(s.DB, "INSERT INTO projects(slug,name,path,brain,added_at) VALUES ('brain','Brain',?,1,?)", root, testNow)
				}
				dir := filepath.Join(root, "docs", "evolution")
				if err := os.MkdirAll(dir, 0o755); err != nil {
					t.Fatal(err)
				}
				var err error
				if tc.name == "project dangling stop" {
					err = os.Symlink("missing-stop-note", filepath.Join(dir, "STOP"))
				} else {
					err = os.WriteFile(filepath.Join(dir, "STOP"), []byte("owner stop"), 0o644)
				}
				if err != nil {
					t.Fatal(err)
				}
			case "recovery off":
				setRecoverSetting(t, s, "daemon.recover", "off")
			case "cap":
				setRecoverSetting(t, s, "daemon.recover_limit", "0")
			case "backoff":
				exec1(s.DB, "UPDATE jobs SET finished_at = ? WHERE id = ?", plusMinutes(testNow, -2), id)
			case "history":
				exec1(s.DB, "UPDATE jobs SET finished_at = ? WHERE id = ?", plusHours(testNow, -24*30), id)
			case "bad timestamp":
				exec1(s.DB, "UPDATE jobs SET finished_at = '' WHERE id = ?", id)
			case "tier 2":
				exec1(s.DB, "UPDATE jobs SET tier = 2 WHERE id = ?", id)
			case "lead", "baseline", "experiment", "answer":
				exec1(s.DB, "UPDATE jobs SET kind = ? WHERE id = ?", tc.name, id)
			case "question":
				_, err := createPost(s, testNow, postOpts{Kind: "question", Project: "proj", Area: "jobs", Thread: id,
					Summary: "Owner approval required", Agent: actorAgent(s, "daemon"), ForOwner: true})
				if err != nil {
					t.Fatal(err)
				}
			case "held":
				recoveryEvent(s.DB, testNow, "daemon", id, "held", failure{Class: failureDeterministic, Reason: "owner review"}, id, 0, "")
			}
			if strings.HasPrefix(tc.name, "task ") {
				exec1(s.DB, "UPDATE jobs SET source = 'task:TASK-1' WHERE id = ?", id)
				if tc.name != "task missing" {
					status := "in_progress"
					if tc.name == "task cancelled" || tc.name == "task done" {
						status = strings.TrimPrefix(tc.name, "task ")
					}
					exec1(s.DB, `INSERT INTO tasks(id,project,title,status,job_id,created_by,created_at,updated_at)
						VALUES ('TASK-1','proj','Task',?,?,'human',?,?)`, status, id, testNow, testNow)
				}
				if tc.name == "task followed up" {
					eventAt(s.DB, testNow, "proj-lead", followUpAction, "TASK-1", "followed up")
				}
			}
			before := jobRow(s.DB, id)
			events := count(t, s, "SELECT COUNT(*) AS n FROM events")
			page, err := ReadFailedJobs(s, FailedJobsQuery{Project: "proj"})
			if err != nil {
				t.Fatal(err)
			}
			if tc.name != "cancelled status" {
				if len(page.Jobs) != 1 || page.Jobs[0].Category != tc.category || page.Jobs[0].Eligible != (tc.category == "eligible") {
					t.Fatalf("view: %+v", page)
				}
				if tc.class != "" && page.Jobs[0].Class != tc.class {
					t.Errorf("class = %s, want %s", page.Jobs[0].Class, tc.class)
				}
				if page.Jobs[0].Reason == "" || page.Jobs[0].Fix == "" {
					t.Error("view has no actionable explanation")
				}
			}
			if n := count(t, s, "SELECT COUNT(*) AS n FROM events"); n != events {
				t.Fatal("GET wrote an event")
			}
			result := repairOne(t, s, id)
			if result.Outcome != tc.outcome || result.Category != tc.category {
				t.Fatalf("repair result = %+v", result)
			}
			if tc.outcome == "skipped" && (!reflect.DeepEqual(before, jobRow(s.DB, id)) || count(t, s, "SELECT COUNT(*) AS n FROM events") != events) {
				t.Fatal("held repair mutated jobs or audit history")
			}
		})
	}
}

func TestJobRepairEvidenceHistoryAndActor(t *testing.T) {
	s, root := repairFixture(t)
	original := repairFailure(t, s, "job-evidence", "verify exit 1")
	log := filepath.Join(root, "evidence.log")
	if err := os.WriteFile(log, []byte("exact failing assertion\ngithub_pat_abcdefghijklmnopqrstuvwxyz"), 0o600); err != nil {
		t.Fatal(err)
	}
	run := insertRecoverRun(t, s, original.str("id"), "run-evidence", "failed", "verify exit 1", log, "")
	repaired := repairOne(t, s, original.str("id"))
	if repaired.Outcome != "replanned" || repaired.CarrierID == original.str("id") {
		t.Fatalf("repair = %+v", repaired)
	}
	copy := jobRow(s.DB, repaired.CarrierID)
	for _, want := range []string{original.str("brief"), "verify exit 1", "run-evidence", "exact failing assertion", "[redacted]", "Never weaken a check"} {
		if !strings.Contains(copy.str("brief"), want) {
			t.Errorf("corrected brief lacks %q", want)
		}
	}
	if strings.Contains(copy.str("brief"), "github_pat_abcdefghijklmnopqrstuvwxyz") {
		t.Fatal("corrected brief disclosed a token-shaped value")
	}
	if !reflect.DeepEqual(original, jobRow(s.DB, original.str("id"))) || !reflect.DeepEqual(run, lastRecoveryRun(s.DB, original.str("id"))) {
		t.Fatal("repair rewrote historical failure evidence")
	}
	for _, e := range mustRows(s.DB, "SELECT * FROM events WHERE action = ? OR subject = ?", board.JobRecoverAction, copy.str("id")) {
		if e.str("actor") != store.HumanAgent {
			t.Fatalf("audit actor = %s", e.str("actor"))
		}
	}
	if copy.str("created_by") != store.HumanAgent {
		t.Fatal("copy was not attributed to the owner")
	}
	for _, status := range []string{"queued", "done"} {
		exec1(s.DB, "UPDATE jobs SET status = ?, finished_at = ? WHERE id = ?", status, testNow, copy.str("id"))
		again := repairOne(t, s, original.str("id"))
		if again.Outcome != "skipped" || again.Category != "superseded" || again.CarrierID != copy.str("id") {
			t.Fatalf("repeated repair with %s carrier = %+v", status, again)
		}
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs"); n != 2 {
		t.Fatalf("duplicate copies: %d jobs", n)
	}
}

func TestJobRepairCarrierSourceAndLineage(t *testing.T) {
	for _, source := range []string{"", "suggestion:shared"} {
		for _, status := range []string{"queued", "leased", "blocked", "done", "cancelled", "active provisional"} {
			t.Run(source+"/"+status, func(t *testing.T) {
				s, _ := repairFixture(t)
				job := repairFailure(t, s, "job-old", "connection reset")
				carrier := repairFailure(t, s, "job-carrier", "verify exit 1")
				exec1(s.DB, "UPDATE jobs SET source = ?", source)
				// An open job can predate the failure. A done source carrier may
				// have started before the failure but finished after it.
				actual := status
				if actual == "active provisional" {
					actual = "failed"
					insertRecoverRun(t, s, carrier.str("id"), "run-active", "running", "", "", "")
				}
				exec1(s.DB, "UPDATE jobs SET status = ?, finished_at = ? WHERE id = ?", actual, testNow, carrier.str("id"))
				if source == "" {
					recoveryEvent(s.DB, plusHours(testNow, -2), "human", job.str("id"), "replanned",
						failure{Class: failureDeterministic}, job.str("id"), 1, carrier.str("id"))
				}
				got := repairOne(t, s, job.str("id"))
				if got.Outcome != "skipped" || got.Category != "superseded" || got.CarrierID != carrier.str("id") {
					t.Fatalf("carrier ignored: %+v", got)
				}
			})
		}
	}
}

func TestJobRepairCurrentFailureAndConcurrency(t *testing.T) {
	for _, mode := range []string{"same failure", "CLI race", "new failure", "completed", "queued"} {
		t.Run(mode, func(t *testing.T) {
			s, _ := repairFixture(t)
			id := "job-race"
			repairFailure(t, s, id, "verify exit 1")
			if mode == "new failure" {
				recoveryEvent(s.DB, plusHours(testNow, -2), "daemon", id, "requeued", failure{Class: failureTransient}, id, 1, "")
				eventAt(s.DB, plusMinutes(testNow, -30), "worker", "job.failed", id, "fresh failure")
			}
			if mode == "completed" || mode == "queued" {
				status := "done"
				if mode == "queued" {
					status = "queued"
				}
				exec1(s.DB, "UPDATE jobs SET status = ? WHERE id = ?", status, id)
			}
			var wg sync.WaitGroup
			for i := range 2 {
				wg.Go(func() {
					if mode == "CLI race" && i == 1 {
						if _, err := board.RetryJob(s, board.RetryInput{JobID: id, AsNew: true}); err != nil {
							t.Error(err)
						}
					} else if _, err := RepairFailedJobs(s, RepairJobsInput{JobIDs: []string{id}}); err != nil {
						t.Error(err)
					}
				})
			}
			wg.Wait()
			want := 1
			if mode == "completed" {
				want = 0
			}
			if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE status = 'queued'"); n != want {
				t.Fatalf("queued = %d, want %d", n, want)
			}
		})
	}
}

func TestJobRepairQueueBoundAndSelection(t *testing.T) {
	s, _ := repairFixture(t)
	for i := range 6 {
		repairFailure(t, s, fmt.Sprintf("job-%d", i), "verify exit 1")
	}
	events := count(t, s, "SELECT COUNT(*) AS n FROM events")
	for _, ids := range [][]string{nil, {"job-0", "job-1", "job-2", "job-3", "job-4", "job-5"}, {"job-0", "job-0"}, {""}, {" job-0"}} {
		if out, err := RepairFailedJobs(s, RepairJobsInput{JobIDs: ids}); err == nil || out != nil {
			t.Fatalf("invalid selection accepted: %v %+v %v", ids, out, err)
		}
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events"); n != events {
		t.Fatal("invalid selection mutated the ledger")
	}
	out, err := RepairFailedJobs(s, RepairJobsInput{JobIDs: []string{"job-0", "job-1", "job-2", "job-3", "job-4"}})
	if err != nil || out.Queued != 5 || out.Errors != 0 {
		t.Fatalf("bounded selection: %+v %v", out, err)
	}
	got := repairOne(t, s, "job-5")
	if got.Outcome != "skipped" || got.Category != "queue_full" {
		t.Fatalf("queue overflow: %+v", got)
	}
}

func TestJobRepairConcurrentQueueBound(t *testing.T) {
	s, _ := repairFixture(t)
	for i := range 6 {
		id := fmt.Sprintf("job-%d", i)
		repairFailure(t, s, id, "verify exit 1")
		if i < 4 {
			exec1(s.DB, "UPDATE jobs SET status = 'queued' WHERE id = ?", id)
		}
	}
	var wg sync.WaitGroup
	for _, id := range []string{"job-4", "job-5"} {
		wg.Go(func() {
			if _, err := RepairFailedJobs(s, RepairJobsInput{JobIDs: []string{id}}); err != nil {
				t.Error(err)
			}
		})
	}
	wg.Wait()
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE status = 'queued'"); n != recoverQueueRoom {
		t.Fatalf("concurrent repairs exceeded queue bound: %d", n)
	}
}

func TestJobRepairProjectScopeAndPartialErrors(t *testing.T) {
	s, _ := repairFixture(t)
	exec1(s.DB, "INSERT INTO projects(slug,name,path,added_at) VALUES ('other','Other',?,?)", t.TempDir(), testNow)
	for _, id := range []string{"job-good", "job-error", "job-other"} {
		repairFailure(t, s, id, "verify exit 1")
	}
	exec1(s.DB, "UPDATE jobs SET project = 'other' WHERE id = 'job-other'")
	if _, err := RepairFailedJobs(s, RepairJobsInput{Project: "unknown", JobIDs: []string{"job-good"}}); err == nil {
		t.Fatal("unknown project accepted")
	}
	if _, err := ReadFailedJobs(s, FailedJobsQuery{Project: "unknown"}); err == nil {
		t.Fatal("unknown read project accepted")
	}
	exec1(s.DB, `CREATE TRIGGER reject_one_repair BEFORE INSERT ON events
		WHEN NEW.action = 'job.recover' AND NEW.subject = 'job-error'
		BEGIN SELECT RAISE(ABORT, 'injected ledger failure'); END`)
	out, err := RepairFailedJobs(s, RepairJobsInput{Project: "proj", JobIDs: []string{"job-good", "missing", "job-other", "job-error"}})
	if err != nil || out.Queued != 1 || out.Skipped != 2 || out.Errors != 1 {
		t.Fatalf("partial result: %+v %v", out, err)
	}
	for _, i := range []int{1, 2} {
		if out.Results[i].Category != "not_found" || out.Results[i].CarrierID != "" {
			t.Fatalf("scope leak: %+v", out.Results[i])
		}
	}
	if !strings.Contains(out.Results[3].Reason, "injected ledger failure") || out.Results[3].CarrierID != "" {
		t.Fatalf("failed write claimed success: %+v", out.Results[3])
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs"); n != 4 {
		t.Fatalf("orphan corrected copy after rollback: %d", n)
	}
	exec1(s.DB, "DROP TRIGGER reject_one_repair")
	if got := repairOne(t, s, "job-error"); got.Outcome != "replanned" {
		t.Fatalf("error was not retryable: %+v", got)
	}
}

func TestJobRepairAllHistoryCountAndPagination(t *testing.T) {
	s, _ := repairFixture(t)
	if err := immediate(s, func(q querier) error {
		for i := range 1644 {
			exec1(q, `INSERT INTO jobs(id,project,kind,tier,title,status,created_at,finished_at,result)
				VALUES (?,'proj','improve',1,'Historical failure','failed',?,?,'verify exit 1')`,
				fmt.Sprintf("job-%04d", i), plusHours(testNow, -24*31), plusHours(testNow, -24*30))
		}
		return nil
	}); err != nil {
		t.Fatal(err)
	}
	events := count(t, s, "SELECT COUNT(*) AS n FROM events")
	first, err := ReadFailedJobs(s, FailedJobsQuery{Project: "proj", Limit: 7})
	if err != nil || first.HistoricalTotal != 1644 || len(first.Jobs) != 7 || first.NextBefore == "" || first.PageCounts["outside_window"] != 7 {
		t.Fatalf("first page: %+v %v", first, err)
	}
	next, err := ReadFailedJobs(s, FailedJobsQuery{Project: "proj", Limit: 7, Before: first.NextBefore})
	if err != nil || next.HistoricalTotal != 1644 || len(next.Jobs) != 7 || next.Jobs[0].ID == first.Jobs[0].ID {
		t.Fatalf("next page: %+v %v", next, err)
	}
	if first.Jobs[6].ID <= next.Jobs[0].ID {
		t.Fatal("tied timestamps were not paginated by ID")
	}
	for _, query := range []FailedJobsQuery{{Limit: -1}, {Limit: FailedJobsMaxLimit + 1}, {Before: "not-a-cursor"}} {
		if _, err := ReadFailedJobs(s, query); err == nil {
			t.Fatalf("unbounded or invalid page accepted: %+v", query)
		}
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM events"); n != events {
		t.Fatal("history reads wrote recovery decisions")
	}
}

func TestJobRepairRechecksDisplayedFailure(t *testing.T) {
	for _, change := range []string{"owner cancellation", "structural result", "active run", "STOP", "inactive project"} {
		t.Run(change, func(t *testing.T) {
			s, _ := repairFixture(t)
			repairFailure(t, s, "job-changed", "verify exit 1")
			page, err := ReadFailedJobs(s, FailedJobsQuery{Project: "proj"})
			if err != nil || len(page.Jobs) != 1 || !page.Jobs[0].Eligible {
				t.Fatalf("initial eligibility: %+v %v", page, err)
			}
			switch change {
			case "owner cancellation":
				exec1(s.DB, "UPDATE jobs SET result = 'cancelled by the owner' WHERE id = 'job-changed'")
			case "structural result":
				exec1(s.DB, "UPDATE jobs SET result = 'verify exit 127' WHERE id = 'job-changed'")
			case "active run":
				insertRecoverRun(t, s, "job-changed", "run-changed", "running", "", "", "")
			case "STOP":
				setRecoverSetting(t, s, "stop", "1")
			case "inactive project":
				exec1(s.DB, "UPDATE projects SET status = 'inactive' WHERE slug = 'proj'")
			}
			if result := repairOne(t, s, "job-changed"); result.Outcome != "skipped" {
				t.Fatalf("stale eligibility used: %+v", result)
			}
			if count(t, s, "SELECT COUNT(*) AS n FROM jobs") != 1 {
				t.Fatal("changed failure was copied")
			}
		})
	}
}

func TestJobRepairSourceScopeAndTaskCarrier(t *testing.T) {
	s, _ := repairFixture(t)
	exec1(s.DB, "INSERT INTO projects(slug,name,path,added_at) VALUES ('other','Other',?,?)", t.TempDir(), testNow)
	repairFailure(t, s, "job-source", "verify exit 1")
	repairFailure(t, s, "job-other", "verify exit 1")
	exec1(s.DB, "UPDATE jobs SET source = 'shared-source'")
	exec1(s.DB, "UPDATE jobs SET project = 'other', status = 'queued' WHERE id = 'job-other'")
	repaired := repairOne(t, s, "job-source")
	if repaired.Outcome != "replanned" || jobRow(s.DB, repaired.CarrierID).str("project") != "proj" {
		t.Fatalf("cross-project source collision: %+v", repaired)
	}

	repairFailure(t, s, "job-task-old", "connection reset")
	repairFailure(t, s, "job-task-current", "verify exit 1")
	exec1(s.DB, "UPDATE jobs SET source = 'task:TASK-1' WHERE id = 'job-task-old'")
	exec1(s.DB, `INSERT INTO tasks(id,project,title,status,job_id,created_by,created_at,updated_at)
		VALUES ('TASK-1','proj','Task','in_progress','job-task-current','human',?,?)`, testNow, testNow)
	got := repairOne(t, s, "job-task-old")
	if got.Outcome != "skipped" || got.CarrierID != "job-task-current" || got.CarrierStatus != "failed" {
		t.Fatalf("current task carrier missing: %+v", got)
	}
}

func TestJobRepairInvalidGatesFailClosed(t *testing.T) {
	for _, gate := range []string{"stop", "daemon.recover", "daemon.recover_days", "daemon.recover_limit", "checkout path"} {
		t.Run(gate, func(t *testing.T) {
			s, _ := repairFixture(t)
			repairFailure(t, s, "job-invalid-gate", "verify exit 1")
			if gate == "checkout path" {
				exec1(s.DB, "UPDATE projects SET path = '' WHERE slug = 'proj'")
			} else {
				exec1(s.DB, "INSERT OR REPLACE INTO settings(name,value,updated_at,updated_by) VALUES (?,'invalid',?,'human')", gate, testNow)
			}
			if _, err := ReadFailedJobs(s, FailedJobsQuery{Project: "proj"}); err == nil {
				t.Fatal("invalid gate was silently ignored")
			}
			if got := repairOne(t, s, "job-invalid-gate"); got.Outcome != "error" || got.Reason == "" {
				t.Fatalf("invalid gate queued work: %+v", got)
			}
			if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs"); n != 1 {
				t.Fatal("invalid gate mutated jobs")
			}
		})
	}
}

func repairStoreAt(t *testing.T, s *store.Store, hours float64) *store.Store {
	t.Helper()
	later, err := store.Open(store.Options{Home: s.Home, Now: plusHours(testNow, hours), SkipRecords: true})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = later.Close() })
	return later
}

func TestJobRepairCancelledAncestorIntent(t *testing.T) {
	for _, source := range []string{"", "manual:cancelled-chain"} {
		for _, mode := range []string{"earlier cancellation", "earlier same second", "newer same second", "newer before failure", "newer after failure", "API is not an override", "agent is not an override"} {
			t.Run(source+"/"+mode, func(t *testing.T) {
				s, _ := repairFixture(t)
				root := repairFailure(t, s, "job-cancelled-root", "verify exit 1")
				exec1(s.DB, "UPDATE jobs SET source = ? WHERE id = ?", source, root.str("id"))
				if _, err := board.FinishJob(s, root.str("id"), store.HumanAgent, "cancelled", "cancelled by the owner", "", nil); err != nil {
					t.Fatal(err)
				}
				intentHour, failureHour := 1.0, 2.0
				if mode == "earlier same second" {
					intentHour, failureHour = 0, 0
				}
				var child *board.Job
				intentStore := repairStoreAt(t, s, intentHour)
				if mode == "API is not an override" {
					copy, _, err := board.AddJob(intentStore, board.JobInput{Project: "proj", Kind: "improve", Tier: 1,
						Title: "Corrected attempt", Brief: root.str("brief"), Source: source, CreatedBy: store.HumanAgent})
					if err != nil {
						t.Fatal(err)
					}
					child = copy
					recoveryEvent(s.DB, intentStore.Now(), store.HumanAgent, root.str("id"), "replanned",
						failure{Class: failureDeterministic, Reason: "verify exit 1"}, root.str("id"), 1, child.ID)
				} else {
					actor := store.HumanAgent
					if mode == "agent is not an override" {
						actor = "worker"
					}
					retry, err := board.RetryJob(intentStore, board.RetryInput{JobID: root.str("id"), AsNew: true, Actor: actor})
					if err != nil {
						t.Fatal(err)
					}
					child = retry.Job
				}
				switch mode {
				case "newer same second", "newer before failure", "newer after failure":
					at := 1.5
					if mode == "newer same second" {
						at = intentHour
					} else if mode == "newer after failure" {
						at = 2.5
					}
					if _, err := board.FinishJob(repairStoreAt(t, s, at), root.str("id"), store.HumanAgent, "cancelled", "cancelled by the owner again", "", nil); err != nil {
						t.Fatal(err)
					}
				}
				if _, err := board.FinishJob(repairStoreAt(t, s, failureHour), child.ID, store.HumanAgent, "failed", "verify exit 1", "", nil); err != nil {
					t.Fatal(err)
				}
				before := jobRow(s.DB, root.str("id"))
				later := repairStoreAt(t, s, 3)
				page, err := ReadFailedJobs(later, FailedJobsQuery{Project: "proj"})
				if err != nil || len(page.Jobs) != 1 {
					t.Fatalf("child view: %+v %v", page, err)
				}
				authorized := strings.HasPrefix(mode, "earlier")
				if page.Jobs[0].Eligible != authorized {
					t.Fatalf("cancellation chronology was lost: %+v", page.Jobs[0])
				}
				if got := repairOne(t, later, root.str("id")); got.Outcome != "skipped" || got.Category != "cancelled" {
					t.Fatalf("API undid root cancellation: %+v", got)
				}
				if !authorized {
					if _, err := board.RetryJob(later, board.RetryInput{JobID: child.ID, AsNew: true, Actor: store.HumanAgent}); err == nil || !strings.Contains(err.Error(), root.str("id")) {
						t.Fatalf("CLI ignored cancellation: %v", err)
					}
				}
				got := repairOne(t, later, child.ID)
				if !authorized {
					if got.Outcome != "skipped" || got.Category != "superseded" || got.CarrierID != root.str("id") {
						t.Fatalf("cancellation was overridden: %+v", got)
					}
				} else {
					if got.Outcome != "replanned" {
						t.Fatalf("later authorized child remained blocked: %+v", got)
					}
					if _, err := board.FinishJob(repairStoreAt(t, s, 4), got.CarrierID, store.HumanAgent, "failed", "verify exit 1", "", nil); err != nil {
						t.Fatal(err)
					}
					if next := repairOne(t, repairStoreAt(t, s, 5), got.CarrierID); next.Outcome != "replanned" {
						t.Fatalf("authorization did not follow the corrected descendant: %+v", next)
					}
				}
				if !reflect.DeepEqual(before, jobRow(s.DB, root.str("id"))) {
					t.Fatal("cancelled ancestor history was changed")
				}
			})
		}
	}
}

func TestJobRepairAuthorizedChainStillDedupesCarriers(t *testing.T) {
	for _, source := range []string{"", "manual:cancelled-chain"} {
		for _, status := range []string{"queued", "leased", "blocked", "done", "cancelled", "active provisional"} {
			t.Run(source+"/"+status, func(t *testing.T) {
				s, _ := repairFixture(t)
				root := repairFailure(t, s, "job-cancelled-root", "verify exit 1")
				exec1(s.DB, "UPDATE jobs SET source = ? WHERE id = ?", source, root.str("id"))
				if _, err := board.FinishJob(s, root.str("id"), store.HumanAgent, "cancelled", "cancelled by the owner", "", nil); err != nil {
					t.Fatal(err)
				}
				child, err := board.RetryJob(repairStoreAt(t, s, 1), board.RetryInput{JobID: root.str("id"), AsNew: true, Actor: store.HumanAgent})
				if err != nil {
					t.Fatal(err)
				}
				if _, err := board.FinishJob(repairStoreAt(t, s, 2), child.Job.ID, store.HumanAgent, "failed", "verify exit 1", "", nil); err != nil {
					t.Fatal(err)
				}
				later := repairStoreAt(t, s, 3)
				repaired := repairOne(t, later, child.Job.ID)
				if repaired.Outcome != "replanned" {
					t.Fatalf("initial recovery: %+v", repaired)
				}
				actual := status
				if status == "active provisional" {
					actual = "failed"
					insertRecoverRun(t, s, repaired.CarrierID, "run-current", "running", "", "", "")
				}
				exec1(s.DB, "UPDATE jobs SET status = ?, finished_at = ? WHERE id = ?", actual, plusHours(testNow, 4), repaired.CarrierID)
				events := count(t, s, "SELECT COUNT(*) AS n FROM events")
				got := repairOne(t, repairStoreAt(t, s, 5), child.Job.ID)
				if got.Outcome != "skipped" || got.Category != "superseded" || got.CarrierID != repaired.CarrierID {
					t.Fatalf("current %s carrier was ignored: %+v", status, got)
				}
				if count(t, s, "SELECT COUNT(*) AS n FROM jobs") != 3 || count(t, s, "SELECT COUNT(*) AS n FROM events") != events {
					t.Fatal("deduped recovery mutated jobs or audit history")
				}
			})
		}
	}
}

func TestJobRepairCancellationOverrideDoesNotAuthorizeSibling(t *testing.T) {
	for _, source := range []string{"", "manual:cancelled-branches"} {
		name := "without source"
		if source != "" {
			name = "with source"
		}
		t.Run(name, func(t *testing.T) {
			s, _ := repairFixture(t)
			root := repairFailure(t, s, "job-root", "verify exit 1")
			exec1(s.DB, "UPDATE jobs SET source = ? WHERE id = ?", source, root.str("id"))
			oldChild, err := board.RetryJob(s, board.RetryInput{JobID: root.str("id"), AsNew: true, Actor: store.HumanAgent})
			if err != nil {
				t.Fatal(err)
			}
			if _, err := board.FinishJob(repairStoreAt(t, s, 0.5), oldChild.Job.ID, store.HumanAgent, "failed", "verify exit 1", "", nil); err != nil {
				t.Fatal(err)
			}
			if _, err := board.FinishJob(repairStoreAt(t, s, 1), root.str("id"), store.HumanAgent, "cancelled", "cancelled by the owner", "", nil); err != nil {
				t.Fatal(err)
			}
			newChild, err := board.RetryJob(repairStoreAt(t, s, 2), board.RetryInput{JobID: root.str("id"), AsNew: true, Actor: store.HumanAgent})
			if err != nil {
				t.Fatal(err)
			}
			if _, err := board.FinishJob(repairStoreAt(t, s, 3), newChild.Job.ID, store.HumanAgent, "failed", "verify exit 1", "", nil); err != nil {
				t.Fatal(err)
			}
			later := repairStoreAt(t, s, 4)
			if got := repairOne(t, later, oldChild.Job.ID); got.Outcome != "skipped" || got.CarrierID != root.str("id") {
				t.Fatalf("new retry authorized the cancelled sibling branch: %+v", got)
			}
			if got := repairOne(t, later, newChild.Job.ID); got.Outcome != "replanned" {
				t.Fatalf("explicitly authorized branch stayed held: %+v", got)
			}
		})
	}
}

func TestRecoverCancelledAncestorAfterHumanRetry(t *testing.T) {
	for _, source := range []string{"", "manual:cancelled-chain"} {
		name := "without source"
		if source != "" {
			name = "with source"
		}
		t.Run(name, func(t *testing.T) {
			s, _ := repairFixture(t)
			root := repairFailure(t, s, "job-root", "verify exit 1")
			exec1(s.DB, "UPDATE jobs SET source = ? WHERE id = ?", source, root.str("id"))
			if _, err := board.FinishJob(s, root.str("id"), store.HumanAgent, "cancelled", "cancelled by the owner", "", nil); err != nil {
				t.Fatal(err)
			}
			child, err := board.RetryJob(repairStoreAt(t, s, 1), board.RetryInput{JobID: root.str("id"), AsNew: true, Actor: store.HumanAgent})
			if err != nil {
				t.Fatal(err)
			}
			if _, err := board.FinishJob(repairStoreAt(t, s, 2), child.Job.ID, store.HumanAgent, "failed", "verify exit 1", "", nil); err != nil {
				t.Fatal(err)
			}
			later := repairStoreAt(t, s, 3)
			project := projectBySlug(s.DB, "proj")
			lines := recoverFailedJobs(later, project, later.Now(), "daemon")
			if len(lines) != 1 || !strings.Contains(lines[0], "re-planned "+child.Job.ID) {
				t.Fatalf("daemon rejected the authorized child: %v", lines)
			}
			carrier := recoverDetail(t, s, child.Job.ID)["job"].(string)
			if _, err := board.FinishJob(repairStoreAt(t, s, 4), root.str("id"), store.HumanAgent, "cancelled", "cancelled by the owner again", "", nil); err != nil {
				t.Fatal(err)
			}
			if _, err := board.FinishJob(repairStoreAt(t, s, 5), carrier, store.HumanAgent, "failed", "verify exit 1", "", nil); err != nil {
				t.Fatal(err)
			}
			later = repairStoreAt(t, s, 6)
			recoverFailedJobs(later, project, later.Now(), "daemon")
			detail := recoverDetail(t, s, carrier)
			if detail["outcome"] != "held" || detail["job"] != root.str("id") || count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE status = 'queued'") != 0 {
				t.Fatalf("daemon ignored the newer cancellation: %#v", detail)
			}
		})
	}
}
