package runner

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/store"
)

func measuredKeepFixture(t *testing.T) (*store.Store, row, row, string, *int) {
	t.Helper()
	s := newStore(t)
	project, root := newRepo(t, s)
	evolvePlanReady(t, s)
	evolveSuggestion(t, s)
	planned, err := evolve.PlanExperiment(s, "proj", evolve.PlanOptions{Agent: "voter-one"})
	if err != nil || !planned.Created {
		t.Fatalf("plan fake experiment: %+v (%v)", planned, err)
	}
	job := jobRow(s.DB, planned.JobID)
	run(t, root, "git", "switch", "-q", "-c", job.str("branch"))
	if err := os.WriteFile(filepath.Join(root, "README.md"), []byte("# measured variant\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	run(t, root, "git", "commit", "-q", "-am", "fake experiment variant")
	run(t, root, "git", "switch", "-q", "main")
	exec1(s.DB, "INSERT INTO runs (id, job_id, project, agent, started_at, status) VALUES (?, ?, 'proj', 'proposer', ?, 'done')",
		"run-"+job.str("id"), job.str("id"), testNow)
	exec1(s.DB, "UPDATE jobs SET status = 'done', finished_at = ? WHERE id = ?", testNow, job.str("id"))
	project["verify_cmd"] = "printf verified"
	exec1(s.DB, "UPDATE projects SET verify_cmd = ? WHERE slug = 'proj'", project.str("verify_cmd"))
	oldMeasure := evolve.Measure
	calls := new(int)
	evolve.Measure = func(_ context.Context, _ *store.Store, slug, _, ref string, _ evolve.MeasureOptions) (*evolve.Scores, error) {
		*calls++
		score := .9
		if ref == "main" {
			score = .8
		}
		return &evolve.Scores{Project: slug, Total: score}, nil
	}
	t.Cleanup(func() { evolve.Measure = oldMeasure })
	return s, project, jobRow(s.DB, planned.JobID), root, calls
}

func deferMeasuredKeep(t *testing.T, s *store.Store, project, job row, root string) {
	t.Helper()
	setSetting(t, s, "landing.auto_stash", "off")
	if err := os.WriteFile(filepath.Join(root, "README.md"), []byte("# unrelated local edit\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	state, note := landExperiment(s, project, job, "daemon", testNow, true)
	if state != "deferred" || !strings.Contains(note, "archive B-002") {
		t.Fatalf("fake measured keep did not defer landing: %s: %s", state, note)
	}
}

func TestMissingMeasuredKeepIsNotDelivered(t *testing.T) {
	s, project, job, root, calls := measuredKeepFixture(t)
	deferMeasuredKeep(t, s, project, job, root)
	run(t, root, "git", "branch", "-D", job.str("branch"))
	state, note := landExperiment(s, project, job, "daemon", testNow, true)
	if state != "missing" {
		t.Fatalf("missing kept artifact: %s: %s", state, note)
	}
	current := jobRow(s.DB, job.str("id"))
	if current.str("status") != "failed" || current.str("branch") != "" {
		t.Errorf("missing kept artifact was not terminal non-success: %v", current)
	}
	if feedback, err := evolve.PlanFeedback(s, "proj", "daemon"); err != nil || feedback != nil {
		t.Errorf("missing kept branch produced successful measured feedback: %+v (%v)", feedback, err)
	}
	archives, err := evolve.ListArchive(s, "proj")
	if err != nil || len(archives) != 2 || !archives[0].Kept || archives[0].VariantScore != .9 || *calls != 2 {
		t.Fatalf("missing artifact changed the measurement or was measured again: %v, calls=%d (%v)", archives, *calls, err)
	}
	status, err := evolve.Status(s, "proj")
	if err != nil || status.Cycles != 1 {
		t.Fatalf("delivery failure rewrote the measured-cycle count: %+v (%v)", status, err)
	}
}

func TestMeasuredLandingRecordsCurrentArchive(t *testing.T) {
	for _, path := range []string{"landed", "resumed", "merged"} {
		t.Run(path, func(t *testing.T) {
			s, project, job, root, calls := measuredKeepFixture(t)
			if path != "landed" {
				deferMeasuredKeep(t, s, project, job, root)
				if err := os.WriteFile(filepath.Join(root, "README.md"), []byte("# proj\n"), 0o644); err != nil {
					t.Fatal(err)
				}
				if path == "merged" {
					run(t, root, "git", "merge", "--ff-only", job.str("branch"))
				}
			}
			state, note := landExperiment(s, project, job, "daemon", testNow, true)
			wantState := "landed"
			if path == "merged" {
				wantState = "merged"
			}
			if state != wantState {
				t.Fatalf("successful fake landing: %s: %s", state, note)
			}
			if n := count(t, s, `SELECT COUNT(*) AS n FROM events WHERE action = ? AND subject = ?
				AND json_extract(detail, '$.project') = 'proj' AND json_extract(detail, '$.job') = ?
				AND json_extract(detail, '$.archive') = 'B-002' AND json_extract(detail, '$.run_id') = ?`,
				"land."+wantState, job.str("branch"), job.str("id"), lastRunID(s, job.str("id"))); n != 1 {
				t.Errorf("%s left no durable current-run/current-archive landing proof", path)
			}
			for i := range 2 {
				feedback, err := evolve.PlanFeedback(s, "proj", "daemon")
				if err != nil || (feedback != nil) != (i == 0) {
					t.Fatalf("finalized keep reconciliation %d: %+v (%v)", i, feedback, err)
				}
			}
			if *calls != 2 || count(t, s, "SELECT COUNT(*) AS n FROM evolution_archive") != 2 {
				t.Fatal("deferred finalization repeated a measurement or archive")
			}
		})
	}
}
