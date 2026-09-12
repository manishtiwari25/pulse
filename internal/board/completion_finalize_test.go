package board

import (
	"testing"

	"github.com/manishtiwari25/pulse/internal/store"
)

func completionRunFixture(t *testing.T) (*store.Store, *Job, RunJobResult) {
	t.Helper()
	_, s, _ := fixture(t)
	job, _, err := AddJob(s, JobInput{Project: "demo", Kind: "task", Title: "Complete the fixture", CreatedBy: "human"})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.DB.Exec("UPDATE jobs SET status = 'leased', lease_agent = 'alice', generation = 3 WHERE id = ?", job.ID); err != nil {
		t.Fatal(err)
	}
	if _, err := s.DB.Exec("INSERT INTO runs (id, job_id, agent, project, started_at) VALUES ('run-completion', ?, 'alice', 'demo', ?)",
		job.ID, testNow); err != nil {
		t.Fatal(err)
	}
	gen := 3
	job, err = FinishJob(s, job.ID, "alice", "done", "provisional report", "pulse/alice/provisional", &gen)
	if err != nil {
		t.Fatal(err)
	}
	return s, job, RunJobResult{RunID: "run-completion", JobID: job.ID, Agent: "alice", Generation: gen,
		Status: "done", Result: "controller result"}
}

func TestCompletionFinalizeReplacesBranch(t *testing.T) {
	for _, tc := range []struct{ name, status, branch string }{
		{"no artifacts", "done", ""},
		{"verified artifact", "done", "pulse/alice/verified"},
		{"failed", "failed", ""},
		{"requeued experiment", "queued", "evolve/planned"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			s, _, in := completionRunFixture(t)
			in.Status, in.Branch = tc.status, tc.branch
			got, err := FinishRunJob(s, in)
			if err != nil {
				t.Fatal(err)
			}
			if got.Status != tc.status || got.Branch != tc.branch || got.Result != in.Result || got.Generation != in.Generation {
				t.Fatalf("controller outcome: %+v", got)
			}
			if tc.status == "queued" && (got.LeaseAgent != "" || got.FinishedAt != "") {
				t.Fatalf("requeued job kept its lease: %+v", got)
			}
			var runStatus string
			if err := s.DB.QueryRow("SELECT status FROM runs WHERE id = ?", in.RunID).Scan(&runStatus); err != nil || runStatus != "running" {
				t.Fatalf("job finalizer released the controller before run finalization: %s %v", runStatus, err)
			}
		})
	}
}

func TestCompletionFinalizeRejectsStaleOwnership(t *testing.T) {
	for _, name := range []string{"missing run", "finished run", "generation", "agent", "lease owner", "run project", "cancelled", "failed"} {
		t.Run(name, func(t *testing.T) {
			s, _, in := completionRunFixture(t)
			var err error
			switch name {
			case "missing run":
				in.RunID = "missing"
			case "finished run":
				_, err = s.DB.Exec("UPDATE runs SET status = 'done' WHERE id = ?", in.RunID)
			case "generation":
				in.Generation++
			case "agent":
				in.Agent = "bob"
			case "lease owner":
				_, err = s.DB.Exec("UPDATE jobs SET lease_agent = 'bob' WHERE id = ?", in.JobID)
			case "run project":
				_, err = s.DB.Exec("UPDATE runs SET project = 'other' WHERE id = ?", in.RunID)
			case "cancelled", "failed":
				_, err = s.DB.Exec("UPDATE jobs SET status = ? WHERE id = ?", name, in.JobID)
			}
			if err != nil {
				t.Fatal(err)
			}
			before, err := JobByID(s, in.JobID)
			if err != nil {
				t.Fatal(err)
			}
			if _, err := FinishRunJob(s, in); err == nil {
				t.Fatal("stale controller outcome accepted")
			}
			after, err := JobByID(s, in.JobID)
			if err != nil || *before != *after {
				t.Fatalf("rejected completion changed the job: before=%+v after=%+v err=%v", before, after, err)
			}
		})
	}
}

func TestCompletionFinalizeRollsBackOnEventFailure(t *testing.T) {
	s, before, in := completionRunFixture(t)
	if _, err := s.DB.Exec(`CREATE TRIGGER completion_event_failure BEFORE INSERT ON events
		WHEN NEW.action = 'job.done' BEGIN SELECT RAISE(ABORT, 'fixture event failure'); END`); err != nil {
		t.Fatal(err)
	}
	if _, err := FinishRunJob(s, in); err == nil {
		t.Fatal("event failure was swallowed")
	}
	after, err := JobByID(s, in.JobID)
	if err != nil || *before != *after {
		t.Fatalf("failed finalization changed the job: before=%+v after=%+v err=%v", before, after, err)
	}
}
