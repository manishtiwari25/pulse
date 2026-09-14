package runner

import (
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/store"
)

func finishMeasuredExperiment(t *testing.T, s *store.Store, job row, kept bool) string {
	t.Helper()
	runID := "run-" + job.str("id")
	exec1(s.DB, "INSERT INTO runs (id, job_id, agent, project, started_at, status) VALUES (?, ?, 'proposer', 'proj', ?, 'done')",
		runID, job.str("id"), testNow)
	variant := .7
	if kept {
		variant = .9
	}
	suggestion, hypothesis := experimentSuggestion(job)
	id, err := evolve.Archive(s, evolve.ArchiveRow{
		Project: "proj", SuggestionPost: suggestion, Hypothesis: hypothesis, Branch: job.str("branch"),
		Baseline: &evolve.Scores{Total: .8}, Variant: &evolve.Scores{Total: variant},
		Tolerance: .02, Kept: kept, RunID: runID, Note: "fake measured outcome",
	})
	if err != nil {
		t.Fatal(err)
	}
	exec1(s.DB, "UPDATE jobs SET status = 'done', finished_at = ?, result = 'measured; finalization pending' WHERE id = ?",
		testNow, job.str("id"))
	action := "land.dropped"
	if kept {
		action = "land.landed"
	}
	project := mustOne(s.DB, "SELECT * FROM projects WHERE slug = 'proj'")
	if err := recordLanding(s, project, job.str("branch"), "daemon", action, " | fake finalization",
		landOpts{Job: job, ArchiveID: id, RunID: runID}, map[string]any{"fake": true}); err != nil {
		t.Fatal(err)
	}
	return id
}

func TestTendEvolveQueuesMeasuredFeedbackOnce(t *testing.T) {
	resetEvolvePlanReasons()
	s := newStore(t)
	newRepo(t, s)
	evolvePlanReady(t, s)
	evolveSuggestion(t, s)
	exec1(s.DB, `INSERT INTO jobs (id, project, kind, title, status, source, created_at)
		VALUES ('daily-reflect', 'proj', 'reflect', 'Already reflected today', 'done', ?, ?)`,
		"reflect:proj:"+testNow[:10], testNow)

	for cycle, kept := range []bool{true, false} {
		tendEvolve(s, testNow, "daemon", []string{"proj"})
		job := mustOne(s.DB, "SELECT * FROM jobs WHERE kind = 'experiment' AND status = 'queued'")
		if job == nil {
			t.Fatalf("cycle %d: no eligible experiment planned", cycle+1)
		}
		if _, err := planJobs(s, "proj", testNow, "daemon"); err != nil {
			t.Fatal(err)
		}
		if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE source = ? AND kind != 'experiment'", job.str("source")); n != 0 {
			t.Fatalf("ordinary planning consumed cycle %d's reserved suggestion", cycle+1)
		}
		archive := finishMeasuredExperiment(t, s, job, kept)
		tendEvolve(s, testNow, "daemon", []string{"proj"})
		source := "evolve-feedback:proj:" + archive
		feedback := mustRows(s.DB, "SELECT * FROM jobs WHERE source = ?", source)
		if len(feedback) != 1 {
			t.Fatalf("archive %s: got %d per-cycle reflection jobs after finalization; daily reflection must not consume this signal", archive, len(feedback))
		}
		brief := feedback[0].str("brief")
		for _, want := range []string{archive, job.str("id"), "fake measured outcome", "SUGGESTION:", "Hypothesis:", "owner goal", "Product scope"} {
			if !strings.Contains(brief, want) {
				t.Errorf("feedback brief lacks %q:\n%s", want, brief)
			}
		}
		if feedback[0].str("kind") != "reflect" || feedback[0].int("tier") != 0 {
			t.Fatalf("feedback must be a bounded reflection, not an automatic improvement: %v", feedback[0])
		}
		exec1(s.DB, "UPDATE jobs SET status = 'done', finished_at = ? WHERE id = ?", testNow, feedback[0].str("id"))
		resetEvolvePlanReasons()
		tendEvolve(s, testNow, "daemon", []string{"proj"})
		if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE source = ?", source); n != 1 {
			t.Fatalf("archive %s: %d follow-ups after completion and reason-cache reset", archive, n)
		}
		if cycle == 0 {
			next, err := board.CreatePost(s, board.PostInput{
				Kind: "suggestion", Project: "proj", Area: "evolution", Thread: "next-measured-hypothesis",
				Agent: "proposer", Summary: "Try the next README variable", Files: "README.md",
				Body: "Class: operating-layer\nFile: README.md\nVariable: another constant\nHypothesis: test the next measured hypothesis\nTier: 1\n",
			})
			if err != nil {
				t.Fatal(err)
			}
			for _, voter := range []string{"voter-one", "voter-two"} {
				if _, err := board.CreatePost(s, board.PostInput{
					Kind: "vote", Project: "proj", Area: next.Area, Thread: next.Thread, Agent: voter,
					Summary: "independent evidence for the next hypothesis", On: next.ID, Vote: "up",
				}); err != nil {
					t.Fatal(err)
				}
			}
		}
	}
}

func TestTendEvolveFeedbackRequiresFinalizedMeasurement(t *testing.T) {
	for _, state := range []string{"failed", "missing-archive", "landing", "controller-running", "stop", "cap"} {
		t.Run(state, func(t *testing.T) {
			resetEvolvePlanReasons()
			s := newStore(t)
			newRepo(t, s)
			evolvePlanReady(t, s)
			evolveSuggestion(t, s)
			tendEvolve(s, testNow, "daemon", []string{"proj"})
			job := mustOne(s.DB, "SELECT * FROM jobs WHERE kind = 'experiment'")
			if state == "missing-archive" {
				exec1(s.DB, "UPDATE jobs SET status = 'done', branch = '' WHERE id = ?", job.str("id"))
			} else {
				finishMeasuredExperiment(t, s, job, false)
				switch state {
				case "failed":
					exec1(s.DB, "UPDATE jobs SET status = 'failed' WHERE id = ?", job.str("id"))
				case "landing":
					exec1(s.DB, "UPDATE jobs SET branch = ? WHERE id = ?", job.str("branch"), job.str("id"))
				case "controller-running":
					exec1(s.DB, "UPDATE runs SET status = 'running' WHERE job_id = ?", job.str("id"))
				case "stop":
					setSetting(t, s, "stop", "1")
				case "cap":
					setSetting(t, s, "evolve.limits", "cycles=1")
				}
			}
			tendEvolve(s, testNow, "daemon", []string{"proj"})
			if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE source LIKE 'evolve-feedback:%'"); n != 0 {
				t.Fatalf("%s queued %d success-shaped feedback jobs", state, n)
			}
		})
	}
}
