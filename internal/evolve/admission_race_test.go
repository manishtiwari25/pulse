package evolve

import (
	"sync"
	"testing"
	"time"

	"github.com/manishtiwari25/pulse/internal/store"
)

func TestAdmissionRechecksAfterFinalArchive(t *testing.T) {
	for _, kind := range []string{"experiment", "feedback"} {
		for _, closeGate := range []string{"last-cycle", "stop", "evaluator"} {
			t.Run(kind+"/"+closeGate, func(t *testing.T) {
				s := cycleReady(t)
				votedSuggestion(t, s, "first", "operating-layer", "README.md", 3)
				first, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
				if err != nil || !first.Created {
					t.Fatalf("first experiment: %+v (%v)", first, err)
				}
				votedSuggestion(t, s, "next", "operating-layer", "README.md", 2)
				if _, err := s.DB.Exec("UPDATE jobs SET status = 'done' WHERE id = ?", first.JobID); err != nil {
					t.Fatal(err)
				}
				if err := s.SetSetting("human", "evolve.limits", "cycles=1"); err != nil {
					t.Fatal(err)
				}
				observed, release := make(chan *LoopStatus, 1), make(chan struct{})
				old := readPlanPreflight
				readPlanPreflight = func(s *store.Store, project string) (*LoopStatus, error) {
					status, err := old(s, project)
					if err != nil {
						return status, err
					}
					observed <- status
					<-release
					return status, nil
				}
				done := make(chan error, 1)
				created := false
				go func() {
					if kind == "experiment" {
						result, err := PlanExperiment(s, "demo", PlanOptions{Agent: "tester"})
						created = result.Created
						done <- err
					} else {
						job, err := PlanFeedback(s, "demo", "tester")
						created = job != nil
						done <- err
					}
				}()
				released, joined := false, false
				defer func() {
					if !released {
						close(release)
					}
					if !joined {
						select {
						case <-done:
						case <-time.After(5 * time.Second):
							t.Error("admission did not finish after the test barrier was released")
						}
					}
					readPlanPreflight = old
				}()
				select {
				case status := <-observed:
					if status.Level != "D" || status.Cycles != 0 {
						t.Fatalf("preflight was not the last available cycle: %+v", status)
					}
				case <-time.After(3 * time.Second):
					t.Fatal("planner never reached the preflight barrier")
				}
				if err := fakeLanding(s, first.JobID, 0); err != nil {
					t.Fatal(err)
				}
				if closeGate != "last-cycle" {
					if err := s.SetSetting("human", "evolve.limits", "none"); err != nil {
						t.Fatal(err)
					}
				}
				if closeGate == "stop" {
					if err := s.SetSetting("human", "stop", "1"); err != nil {
						t.Fatal(err)
					}
				}
				if closeGate == "evaluator" {
					if err := SetFixtureEnabled(s, "demo", "F01", false, "human"); err != nil {
						t.Fatal(err)
					}
				}
				close(release)
				released = true
				select {
				case err := <-done:
					joined = true
					if err != nil {
						t.Fatal(err)
					}
				case <-time.After(5 * time.Second):
					t.Fatal("admission did not finish")
				}
				if created {
					t.Errorf("%s queued after %s closed between Level D preflight and admission", kind, closeGate)
				}
				if n := countRows(t, s, "jobs WHERE kind = 'experiment'"); n != 1 {
					t.Errorf("admission exceeded the experiment reservation: %d jobs", n)
				}
				if n := countRows(t, s, "jobs WHERE source LIKE 'evolve-feedback:%'"); n != 0 {
					t.Errorf("closed gates admitted %d feedback jobs", n)
				}
			})
		}
	}
}

func TestAdmissionSerializesExperimentCallers(t *testing.T) {
	s := cycleReady(t)
	candidate := votedSuggestion(t, s, "candidate", "operating-layer", "README.md", 2)
	if err := s.SetSetting("human", "evolve.limits", "cycles=1"); err != nil {
		t.Fatal(err)
	}
	other, err := store.Open(store.Options{Home: s.Home, Now: s.Now()})
	if err != nil {
		t.Fatal(err)
	}
	defer other.Close()
	type outcome struct {
		result PlanResult
		err    error
	}
	results := make(chan outcome, 4)
	observed, release := make(chan *LoopStatus, 4), make(chan struct{})
	old := readPlanPreflight
	readPlanPreflight = func(s *store.Store, project string) (*LoopStatus, error) {
		status, err := old(s, project)
		observed <- status
		<-release
		return status, err
	}
	var wg sync.WaitGroup
	for i := range 4 {
		caller := s
		if i%2 == 1 {
			caller = other
		}
		wg.Add(1)
		go func() {
			defer wg.Done()
			result, err := PlanExperiment(caller, "demo", PlanOptions{Agent: "tester"})
			results <- outcome{result, err}
		}()
	}
	finished := make(chan struct{})
	go func() { wg.Wait(); close(finished) }()
	released := false
	defer func() {
		if !released {
			close(release)
		}
		<-finished
		readPlanPreflight = old
	}()
	for range 4 {
		select {
		case status := <-observed:
			if status == nil || status.Level != "D" || status.Cycles != 0 {
				t.Fatalf("concurrent caller did not observe the last available cycle: %+v", status)
			}
		case <-time.After(5 * time.Second):
			t.Fatal("concurrent caller never reached preflight")
		}
	}
	close(release)
	released = true
	select {
	case <-finished:
	case <-time.After(8 * time.Second):
		t.Fatal("concurrent experiment admission did not finish")
	}
	created, id := 0, ""
	for range 4 {
		out := <-results
		if out.err != nil {
			t.Fatal(out.err)
		}
		if out.result.Created {
			created++
		}
		if out.result.JobID == "" || (id != "" && out.result.JobID != id) {
			t.Errorf("callers did not share the same reservation: %+v, first=%s", out.result, id)
		}
		id = out.result.JobID
	}
	if created != 1 || countRows(t, s, "jobs WHERE kind = 'experiment'") != 1 {
		t.Fatalf("created %d reservations for one available cycle", created)
	}
	if n := countRows(t, s, "posts WHERE kind = 'opinion' AND summary LIKE 'Experiment card:%'"); n != 1 {
		t.Fatalf("created %d cards for one reservation", n)
	}
	var source string
	if err := s.DB.QueryRow("SELECT source FROM jobs WHERE kind = 'experiment'").Scan(&source); err != nil || source != "suggestion:"+candidate.ID {
		t.Fatalf("reservation lost its exact suggestion source: %s (%v)", source, err)
	}
}
