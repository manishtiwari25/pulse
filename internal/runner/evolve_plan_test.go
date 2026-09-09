package runner

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/manishtiwari25/pulse/internal/board"
	"github.com/manishtiwari25/pulse/internal/evolve"
	"github.com/manishtiwari25/pulse/internal/store"
)

func evolvePlanReady(t *testing.T, s *store.Store) {
	t.Helper()
	setSetting(t, s, "evolve.loop", "1")
	setSetting(t, s, "quorum", "2")
	constitution := "# Constitution\n\n## Editable scope\n" +
		"| File class | Paths | Stage | Autonomy |\n| --- | --- | --- | --- |\n" +
		"| operating | `README.md` | 2 | standing |\n\n## Immutable paths\n" +
		"| Path | Why |\n| --- | --- |\n| `docs/evolution/**` | owner-owned |\n"
	for slug, body := range map[string]string{"constitution": constitution, "evaluators": "Product scope: off\n"} {
		if _, err := s.PutRecord(store.Record{ID: "EVO-" + slug + "-proj", Project: "proj", Kind: "evolution", Slug: slug},
			nil, body, humanAgent, "fixture"); err != nil {
			t.Fatal(err)
		}
	}
	dir := filepath.Join(t.TempDir(), "heldout")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"owner-one", "owner-two"} {
		if err := os.WriteFile(filepath.Join(dir, name+".request"), []byte("secret-free private prompt\n"), 0o600); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(dir, name+".score"), []byte("#!/bin/sh\nprintf '1\\n'\n"), 0o700); err != nil {
			t.Fatal(err)
		}
	}
	setSetting(t, s, "evolve.heldout_dir", dir)
	scores := &evolve.Scores{Project: "proj", Total: .8, Fixtures: []evolve.FixtureScore{{Name: "starter-readme", Kind: "public", Score: .8, Weight: 1}}}
	if _, err := evolve.Archive(s, evolve.ArchiveRow{Project: "proj", Hypothesis: evolve.BaselineHypothesis, Branch: "main",
		Baseline: scores, Variant: scores, Tolerance: .02, Kept: true, RunID: "run-baseline", Note: "baseline measurement"}); err != nil {
		t.Fatal(err)
	}
}

func evolveSuggestion(t *testing.T, s *store.Store) *board.Post {
	t.Helper()
	for _, name := range []string{"proposer", "voter-one", "voter-two"} {
		join(t, s, name)
	}
	post, err := board.CreatePost(s, board.PostInput{Kind: "suggestion", Project: "proj", Area: "evolution", Thread: "daemon-plan",
		Agent: "proposer", Summary: "Tune README", Files: "README.md",
		Body: "Class: operating-layer\nFile: README.md\nVariable: one constant\nHypothesis: README gets clearer\nTier: 1\n"})
	if err != nil {
		t.Fatal(err)
	}
	for _, agent := range []string{"voter-one", "voter-two"} {
		if _, err := board.CreatePost(s, board.PostInput{Kind: "vote", Project: "proj", Area: "evolution", Thread: "daemon-plan",
			Agent: agent, Summary: "measured", On: post.ID, Vote: "up"}); err != nil {
			t.Fatal(err)
		}
	}
	return post
}

func TestTendEvolveLoopOffDoesNothing(t *testing.T) {
	resetEvolvePlanReasons()
	s := newStore(t)
	newRepo(t, s)
	if lines := tendEvolve(s, testNow, "daemon", []string{"proj"}); len(lines) != 0 {
		t.Fatalf("loop off lines: %v", lines)
	}
}

func TestTendEvolvePlansOneExperiment(t *testing.T) {
	resetEvolvePlanReasons()
	s := newStore(t)
	newRepo(t, s)
	evolvePlanReady(t, s)
	suggestion := evolveSuggestion(t, s)
	(&fakeEvolve{scores: map[string]float64{"main": .8}}).install(t)

	lines := tendEvolve(s, testNow, "daemon", []string{"proj"})
	jobs := mustRows(s.DB, "SELECT * FROM jobs WHERE kind = 'experiment'")
	if len(lines) != 1 || len(jobs) != 1 || !strings.Contains(lines[0], "planned experiment "+jobs[0].str("id")+" for proj: Experiment: Tune README") {
		t.Fatalf("planned lines=%v jobs=%v", lines, jobs)
	}
	job := jobs[0]
	if job.str("status") != "queued" || job.str("source") != "suggestion:"+suggestion.ID || job.str("branch") != "evolve/"+suggestion.ID ||
		job.int("priority") != 30 || job.str("created_by") != "daemon" {
		t.Fatalf("experiment job: %v", job)
	}
	if second := tendEvolve(s, testNow, "daemon", []string{"proj"}); len(second) != 0 {
		t.Fatalf("second tick repeated a line: %v", second)
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE kind = 'experiment'"); n != 1 {
		t.Fatalf("experiment jobs after second tick: %d", n)
	}
	if a, err := s.Agent("daemon"); err != nil || a != nil {
		t.Fatalf("tendEvolve created a daemon agent row: %+v %v", a, err)
	}
}

func TestTendEvolveFailedViolationPostsGateOnce(t *testing.T) {
	resetEvolvePlanReasons()
	s := newStore(t)
	newRepo(t, s)
	evolvePlanReady(t, s)
	evolveSuggestion(t, s)
	(&fakeEvolve{scores: map[string]float64{"main": .8}}).install(t)
	job, _, err := board.AddJob(s, board.JobInput{Project: "proj", Kind: "experiment", Title: "failed experiment", Source: "suggestion:old", CreatedBy: "daemon", Tier: 1})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := board.FinishJob(s, job.ID, "daemon", "failed", "constraint violation: hard stop", "", nil); err != nil {
		t.Fatal(err)
	}

	lines := tendEvolve(s, testNow, "daemon", []string{"proj"})
	if len(lines) != 1 || !strings.Contains(lines[0], "owner question") || !strings.Contains(lines[0], job.ID) {
		t.Fatalf("gate line: %v", lines)
	}
	if second := tendEvolve(s, testNow, "daemon", []string{"proj"}); len(second) != 0 {
		t.Fatalf("second gate tick repeated a line: %v", second)
	}
	questions := mustRows(s.DB, "SELECT * FROM posts WHERE kind = 'question' AND area = 'evolution' AND thread = 'loop-gate'")
	if len(questions) != 1 || questions[0].str("summary") != "Evolution stopped; owner review needed" ||
		!strings.Contains(questions[0].str("body"), job.ID) {
		t.Fatalf("questions: %v", questions)
	}
	if !isOwnerAddressedPost(postRow(s.DB, questions[0].str("id"))) {
		t.Fatal("hard-stop question is not marked for the owner")
	}
	if n := count(t, s, "SELECT COUNT(*) AS n FROM jobs WHERE kind = 'experiment'"); n != 1 {
		t.Fatalf("planned while gated: %d experiment jobs", n)
	}
}
